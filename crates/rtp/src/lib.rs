use std::{net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc};

use anyhow::Context;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Server};
use tower::service_fn;
use tracing::{debug, error, info, warn};

pub mod proto {
    tonic::include_proto!("rtp.v1");
}

use proto::{
    CapabilityRequest, CapabilityResponse, CheckRequest, CheckResponse, CreateResourceRequest,
    CreateResourceResponse, DeleteResourceRequest, Resource, UpdateResourceRequest,
    UpdateResourceResponse,
};

type ResourceTransitionPluginClient =
    proto::resource_transition_plugin_client::ResourceTransitionPluginClient<Channel>;

/// Maximum allowed byte length for any single JSON string field in an RTP request.
/// Proto3 strings can be arbitrarily large; this limit prevents excessive memory use
/// during deserialization.
const MAX_JSON_FIELD_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

#[tonic::async_trait]
pub trait Plugin: Send + Sync + 'static {
    async fn create_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource>;

    async fn update_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource>;

    async fn delete_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let _ = (environment_qid, deployment_id, id, inputs, outputs);
        Ok(())
    }

    async fn check(
        &self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        resource: sclc::Resource,
    ) -> anyhow::Result<sclc::Resource> {
        let _ = (environment_qid, deployment_id, id);
        Ok(resource)
    }
}

#[derive(Debug, Clone)]
pub enum Target {
    Tcp(String),
    Unix(PathBuf),
}

#[derive(Debug, Error)]
pub enum TargetParseError {
    #[error("invalid uri: {0}")]
    Uri(#[from] http::uri::InvalidUri),

    #[error("missing uri scheme")]
    MissingScheme,

    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),

    #[error("invalid tcp target: {0}")]
    InvalidTcpAddress(String),

    #[error("invalid unix socket path")]
    InvalidUnixPath,
}

impl FromStr for Target {
    type Err = TargetParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uri: http::Uri = s.parse()?;
        let Some(scheme) = uri.scheme_str() else {
            return Err(TargetParseError::MissingScheme);
        };

        match scheme {
            "unix" => {
                let path = uri.path();
                if path.is_empty() {
                    return Err(TargetParseError::InvalidUnixPath);
                }
                Ok(Target::Unix(PathBuf::from(path)))
            }
            "tcp" | "http" => {
                let Some(authority) = uri.authority() else {
                    return Err(TargetParseError::InvalidTcpAddress(s.to_owned()));
                };
                Ok(Target::Tcp(authority.as_str().to_owned()))
            }
            other => Err(TargetParseError::UnsupportedScheme(other.to_owned())),
        }
    }
}

#[derive(Debug, Error)]
pub enum ServeError {
    #[error("invalid target: {0}")]
    Target(#[from] TargetParseError),

    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid tcp bind address: {0}")]
    InvalidTcpBindAddress(String),
}

#[derive(Debug, Error)]
pub enum DialError {
    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("invalid endpoint uri: {0}")]
    InvalidUri(#[from] http::uri::InvalidUri),

    #[error("capability exchange failed: {0}")]
    CapabilityExchange(#[from] tonic::Status),

    #[error("failed to resolve tcp address `{target}`: {source}")]
    ResolveTcpAddress {
        target: String,
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
// Helpers — JSON parsing & ResourceId validation
// ---------------------------------------------------------------------------

/// Deserialize a JSON string field into `T`, enforcing [`MAX_JSON_FIELD_BYTES`] and
/// logging on failure. Returns a gRPC `InvalidArgument` status on error.
fn parse_json_field<T: serde::de::DeserializeOwned>(
    json: &str,
    field_name: &str,
    resource_type: &str,
    resource_name: &str,
    rpc_name: &str,
) -> Result<T, tonic::Status> {
    if json.len() > MAX_JSON_FIELD_BYTES {
        warn!(
            resource_type,
            resource_name,
            field_name,
            rpc_name,
            len = json.len(),
            max = MAX_JSON_FIELD_BYTES,
            "JSON field exceeds size limit"
        );
        return Err(tonic::Status::invalid_argument(format!(
            "{field_name} exceeds maximum size of {MAX_JSON_FIELD_BYTES} bytes"
        )));
    }
    serde_json::from_str(json).map_err(|error| {
        warn!(
            resource_type,
            resource_name,
            err = %error,
            "invalid {rpc_name} {field_name} payload"
        );
        tonic::Status::invalid_argument(format!("invalid {field_name}"))
    })
}

/// Build a validated [`ids::ResourceId`] from untrusted `type` and `name` strings.
fn validated_resource_id(typ: &str, name: &str) -> Result<ids::ResourceId, tonic::Status> {
    let composite = format!("{typ}:{name}");
    composite.parse::<ids::ResourceId>().map_err(|_| {
        warn!(
            resource_type = typ,
            resource_name = name,
            "invalid resource ID in request"
        );
        tonic::Status::invalid_argument("invalid resource type/name")
    })
}

// ---------------------------------------------------------------------------
// Server implementation
// ---------------------------------------------------------------------------

struct PluginFactory<P, F>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    plugin_fn: Arc<F>,
    _marker: std::marker::PhantomData<fn() -> P>,
}

impl<P, F> Clone for PluginFactory<P, F>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            plugin_fn: Arc::clone(&self.plugin_fn),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<P, F> PluginFactory<P, F>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    fn new(plugin_fn: F) -> Self {
        Self {
            plugin_fn: Arc::new(plugin_fn),
            _marker: std::marker::PhantomData,
        }
    }

    fn make_connection_service(&self) -> PluginConnectionService<P, F> {
        PluginConnectionService {
            plugin: Arc::new(RwLock::new((self.plugin_fn)())),
            factory: self.clone(),
            peer_capabilities: Arc::new(RwLock::new(None)),
        }
    }
}

struct PluginConnectionService<P, F>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    plugin: Arc<RwLock<P>>,
    factory: PluginFactory<P, F>,
    peer_capabilities: Arc<RwLock<Option<CapabilityRequest>>>,
}

impl<P, F> Clone for PluginConnectionService<P, F>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        self.factory.make_connection_service()
    }
}

#[tonic::async_trait]
impl<P, F> proto::resource_transition_plugin_server::ResourceTransitionPlugin
    for PluginConnectionService<P, F>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    async fn exchange_capabilities(
        &self,
        request: tonic::Request<CapabilityRequest>,
    ) -> Result<tonic::Response<CapabilityResponse>, tonic::Status> {
        let capabilities = request.into_inner();
        info!(
            plugin = capabilities.plugin_name.as_str(),
            version = capabilities.plugin_version.as_str(),
            "received RTP capability exchange"
        );
        *self.peer_capabilities.write().await = Some(capabilities);

        Ok(tonic::Response::new(CapabilityResponse {
            protocol_version: String::from("1"),
            capabilities: vec![String::new()],
        }))
    }

    async fn create_resource(
        &self,
        request: tonic::Request<CreateResourceRequest>,
    ) -> Result<tonic::Response<CreateResourceResponse>, tonic::Status> {
        self.ensure_peer_capabilities().await?;

        let request = request.into_inner();
        info!(
            resource_type = request.resource_type.as_str(),
            resource_name = request.resource_name.as_str(),
            environment_qid = request.environment_qid.as_str(),
            deployment_id = request.deployment_id.as_str(),
            "received create_resource RPC"
        );
        let inputs: sclc::Record = parse_json_field(
            &request.resource_inputs_json,
            "resource_inputs_json",
            &request.resource_type,
            &request.resource_name,
            "create_resource",
        )?;
        let resource_id = validated_resource_id(&request.resource_type, &request.resource_name)?;

        let resource = {
            let mut plugin = self.plugin.write().await;
            plugin
                .create_resource(
                    &request.environment_qid,
                    &request.deployment_id,
                    resource_id.clone(),
                    inputs,
                )
                .await
                .map_err(|error| {
                    error!(
                        resource_type = request.resource_type.as_str(),
                        resource_name = request.resource_name.as_str(),
                        err = %error,
                        "plugin create_resource failed"
                    );
                    tonic::Status::internal(error.to_string())
                })?
        };
        info!(
            resource_type = request.resource_type.as_str(),
            resource_name = request.resource_name.as_str(),
            "completed create_resource RPC"
        );
        Ok(tonic::Response::new(CreateResourceResponse {
            resource: Some(encode_resource(resource_id, resource)?),
        }))
    }

    async fn update_resource(
        &self,
        request: tonic::Request<UpdateResourceRequest>,
    ) -> Result<tonic::Response<UpdateResourceResponse>, tonic::Status> {
        self.ensure_peer_capabilities().await?;

        let request = request.into_inner();
        let current = request
            .resource
            .ok_or_else(|| tonic::Status::invalid_argument("missing resource"))?;
        let resource_id = validated_resource_id(&current.r#type, &current.name)?;
        info!(
            resource_type = resource_id.typ.as_str(),
            resource_name = resource_id.name.as_str(),
            environment_qid = request.environment_qid.as_str(),
            deployment_id = request.deployment_id.as_str(),
            "received update_resource RPC"
        );
        let prev_inputs: sclc::Record = parse_json_field(
            &current.inputs_json,
            "resource.inputs_json",
            &resource_id.typ,
            &resource_id.name,
            "update_resource",
        )?;
        let prev_outputs: sclc::Record = parse_json_field(
            &current.outputs_json,
            "resource.outputs_json",
            &resource_id.typ,
            &resource_id.name,
            "update_resource",
        )?;
        let inputs: sclc::Record = parse_json_field(
            &request.inputs_json,
            "inputs_json",
            &resource_id.typ,
            &resource_id.name,
            "update_resource",
        )?;

        let resource = {
            let mut plugin = self.plugin.write().await;
            plugin
                .update_resource(
                    &request.environment_qid,
                    &request.deployment_id,
                    resource_id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
                .map_err(|error| {
                    error!(
                        resource_type = resource_id.typ.as_str(),
                        resource_name = resource_id.name.as_str(),
                        err = %error,
                        "plugin update_resource failed"
                    );
                    tonic::Status::internal(error.to_string())
                })?
        };
        info!(
            resource_type = resource_id.typ.as_str(),
            resource_name = resource_id.name.as_str(),
            "completed update_resource RPC"
        );
        Ok(tonic::Response::new(UpdateResourceResponse {
            resource: Some(encode_resource(resource_id, resource)?),
        }))
    }

    async fn delete_resource(
        &self,
        request: tonic::Request<DeleteResourceRequest>,
    ) -> Result<tonic::Response<()>, tonic::Status> {
        self.ensure_peer_capabilities().await?;

        let request = request.into_inner();
        let current = request
            .resource
            .ok_or_else(|| tonic::Status::invalid_argument("missing resource"))?;
        let resource_id = validated_resource_id(&current.r#type, &current.name)?;
        info!(
            resource_type = resource_id.typ.as_str(),
            resource_name = resource_id.name.as_str(),
            environment_qid = request.environment_qid.as_str(),
            deployment_id = request.deployment_id.as_str(),
            "received delete_resource RPC"
        );
        let inputs: sclc::Record = parse_json_field(
            &current.inputs_json,
            "resource.inputs_json",
            &resource_id.typ,
            &resource_id.name,
            "delete_resource",
        )?;
        let outputs: sclc::Record = parse_json_field(
            &current.outputs_json,
            "resource.outputs_json",
            &resource_id.typ,
            &resource_id.name,
            "delete_resource",
        )?;

        {
            let mut plugin = self.plugin.write().await;
            plugin
                .delete_resource(
                    &request.environment_qid,
                    &request.deployment_id,
                    resource_id.clone(),
                    inputs,
                    outputs,
                )
                .await
                .map_err(|error| {
                    error!(
                        resource_type = resource_id.typ.as_str(),
                        resource_name = resource_id.name.as_str(),
                        err = %error,
                        "plugin delete_resource failed"
                    );
                    tonic::Status::internal(error.to_string())
                })?;
        }

        info!(
            resource_type = resource_id.typ.as_str(),
            resource_name = resource_id.name.as_str(),
            "completed delete_resource RPC"
        );
        Ok(tonic::Response::new(()))
    }

    async fn check(
        &self,
        request: tonic::Request<CheckRequest>,
    ) -> Result<tonic::Response<CheckResponse>, tonic::Status> {
        self.ensure_peer_capabilities().await?;

        let request = request.into_inner();
        let resource = request
            .resource
            .ok_or_else(|| tonic::Status::invalid_argument("missing check resource"))?;
        let id = validated_resource_id(&resource.r#type, &resource.name)?;
        let parsed = decode_resource(resource)?;

        let plugin = self.plugin.read().await;
        let checked = plugin
            .check(
                &request.environment_qid,
                &request.deployment_id,
                id.clone(),
                parsed,
            )
            .await
            .map_err(|error| {
                error!(
                    resource_type = id.typ.as_str(),
                    resource_name = id.name.as_str(),
                    err = %error,
                    "plugin check failed"
                );
                tonic::Status::internal(error.to_string())
            })?;

        Ok(tonic::Response::new(CheckResponse {
            resource: Some(encode_resource(id, checked)?),
        }))
    }
}

impl<P, F> PluginConnectionService<P, F>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    async fn ensure_peer_capabilities(&self) -> Result<(), tonic::Status> {
        if self.peer_capabilities.read().await.is_none() {
            warn!("rejecting RPC before capability exchange");
            return Err(tonic::Status::failed_precondition(
                "capability exchange required",
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct PluginClient {
    inner: ResourceTransitionPluginClient,
    _capabilities: CapabilityResponse,
}

impl PluginClient {
    pub async fn create_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            environment_qid,
            deployment_id,
            "sending create_resource RPC"
        );
        let resource_inputs_json = serde_json::to_string(&inputs)?;
        let response = self
            .inner
            .create_resource(CreateResourceRequest {
                resource_type: id.typ,
                resource_name: id.name,
                resource_inputs_json,
                environment_qid: environment_qid.to_string(),
                deployment_id: deployment_id.to_string(),
            })
            .await
            .map_err(|error| {
                error!(err = %error, "create_resource RPC failed");
                error
            })?
            .into_inner();

        let resource = response.resource.context("missing resource in response")?;
        decode_resource(resource).map_err(Into::into)
    }

    pub async fn update_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        debug!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            environment_qid,
            deployment_id,
            "sending update_resource RPC"
        );
        let current_inputs_json = serde_json::to_string(&prev_inputs)?;
        let current_outputs_json = serde_json::to_string(&prev_outputs)?;
        let inputs_json = serde_json::to_string(&inputs)?;
        let response = self
            .inner
            .update_resource(UpdateResourceRequest {
                resource: Some(Resource {
                    r#type: id.typ,
                    name: id.name,
                    inputs_json: current_inputs_json,
                    outputs_json: current_outputs_json,
                    markers: vec![],
                }),
                inputs_json,
                environment_qid: environment_qid.to_string(),
                deployment_id: deployment_id.to_string(),
            })
            .await
            .map_err(|error| {
                error!(err = %error, "update_resource RPC failed");
                error
            })?
            .into_inner();

        let resource = response.resource.context("missing resource in response")?;
        decode_resource(resource).map_err(Into::into)
    }

    pub async fn delete_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        debug!(
            resource_type = id.typ.as_str(),
            resource_name = id.name.as_str(),
            environment_qid,
            deployment_id,
            "sending delete_resource RPC"
        );
        let inputs_json = serde_json::to_string(&inputs)?;
        let outputs_json = serde_json::to_string(&outputs)?;
        self.inner
            .delete_resource(DeleteResourceRequest {
                resource: Some(Resource {
                    r#type: id.typ,
                    name: id.name,
                    inputs_json,
                    outputs_json,
                    markers: vec![],
                }),
                environment_qid: environment_qid.to_string(),
                deployment_id: deployment_id.to_string(),
            })
            .await
            .map_err(|error| {
                error!(err = %error, "delete_resource RPC failed");
                error
            })?;
        Ok(())
    }

    pub async fn check(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        resource: sclc::Resource,
    ) -> anyhow::Result<sclc::Resource> {
        let response = self
            .inner
            .check(CheckRequest {
                resource: Some(encode_resource(id, resource)?),
                environment_qid: environment_qid.to_string(),
                deployment_id: deployment_id.to_string(),
            })
            .await?
            .into_inner();
        let resource = response
            .resource
            .context("missing resource in check response")?;
        decode_resource(resource).map_err(Into::into)
    }
}

fn encode_marker(marker: &sclc::Marker) -> i32 {
    match marker {
        sclc::Marker::Volatile => proto::Marker::Volatile as i32,
        sclc::Marker::Sticky => proto::Marker::Sticky as i32,
    }
}

fn decode_marker(value: i32) -> Option<sclc::Marker> {
    match proto::Marker::try_from(value) {
        Ok(proto::Marker::Volatile) => Some(sclc::Marker::Volatile),
        Ok(proto::Marker::Sticky) => Some(sclc::Marker::Sticky),
        Err(_) => {
            warn!(marker_value = value, "unknown marker value; dropping");
            None
        }
    }
}

fn encode_resource(
    id: ids::ResourceId,
    resource: sclc::Resource,
) -> Result<Resource, tonic::Status> {
    let inputs_json = serde_json::to_string(&resource.inputs)
        .map_err(|error| tonic::Status::internal(error.to_string()))?;
    let outputs_json = serde_json::to_string(&resource.outputs)
        .map_err(|error| tonic::Status::internal(error.to_string()))?;
    let markers = resource.markers.iter().map(encode_marker).collect();
    Ok(Resource {
        r#type: id.typ,
        name: id.name,
        inputs_json,
        outputs_json,
        markers,
    })
}

fn decode_resource(resource: Resource) -> Result<sclc::Resource, tonic::Status> {
    let inputs: sclc::Record = serde_json::from_str(&resource.inputs_json)
        .map_err(|error| tonic::Status::invalid_argument(error.to_string()))?;
    let outputs: sclc::Record = serde_json::from_str(&resource.outputs_json)
        .map_err(|error| tonic::Status::invalid_argument(error.to_string()))?;
    let markers = resource
        .markers
        .iter()
        .filter_map(|&v| decode_marker(v))
        .collect();
    Ok(sclc::Resource {
        inputs,
        outputs,
        // Dependencies are not transmitted over RTP — they are tracked by the
        // deployment engine (DE) locally and are not relevant to plugin logic.
        dependencies: vec![],
        markers,
    })
}

pub async fn serve<P, F>(target: impl AsRef<str>, plugin_fn: F) -> Result<(), ServeError>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    let target: Target = target.as_ref().parse()?;
    info!(target = ?&target, "starting RTP server");
    let factory = PluginFactory::<P, F>::new(plugin_fn);
    let service = proto::resource_transition_plugin_server::ResourceTransitionPluginServer::new(
        factory.make_connection_service(),
    );

    match target {
        Target::Tcp(addr) => {
            let addr: SocketAddr = addr
                .parse()
                .map_err(|_| ServeError::InvalidTcpBindAddress(addr.clone()))?;
            info!(addr = %addr, "serving RTP over TCP");
            Server::builder().add_service(service).serve(addr).await?;
        }
        Target::Unix(path) => {
            // Remove any stale socket unconditionally. This avoids a TOCTOU race
            // between the exists-check and the remove (the previous code checked
            // `path.exists()` before removing). If no file exists the error is
            // harmless and we ignore it.
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ServeError::Io(e)),
            }
            info!(path = %path.display(), "serving RTP over Unix socket");
            let listener = tokio::net::UnixListener::bind(path)?;
            let incoming = UnixListenerStream::new(listener);

            Server::builder()
                .add_service(service)
                .serve_with_incoming(incoming)
                .await?;
        }
    }

    Ok(())
}

async fn dial_raw(target: Target) -> Result<ResourceTransitionPluginClient, DialError> {
    match target {
        Target::Tcp(addr) => {
            debug!(addr = %addr, "dialing RTP over TCP");
            resolve_tcp_authority(&addr).await?;
            let client = ResourceTransitionPluginClient::connect(format!("http://{addr}")).await?;
            Ok(client)
        }
        Target::Unix(path) => {
            debug!(path = %path.display(), "dialing RTP over Unix socket");
            let endpoint = Endpoint::try_from("http://[::]:50051")?;
            let channel = endpoint
                .connect_with_connector(service_fn(move |_| {
                    let path = path.clone();
                    async move {
                        tokio::net::UnixStream::connect(path)
                            .await
                            .map(TokioIo::new)
                    }
                }))
                .await?;
            Ok(ResourceTransitionPluginClient::new(channel))
        }
    }
}

async fn resolve_tcp_authority(authority: &str) -> Result<(), DialError> {
    tokio::net::lookup_host(authority)
        .await
        .map(|_| ())
        .map_err(|source| DialError::ResolveTcpAddress {
            target: authority.to_owned(),
            source,
        })
}

pub async fn dial(target: Target) -> Result<PluginClient, DialError> {
    info!(target = ?&target, "dialing RTP plugin");
    let mut inner = dial_raw(target).await?;
    let capabilities = inner
        .exchange_capabilities(CapabilityRequest {
            plugin_name: String::from("rtp"),
            plugin_version: env!("CARGO_PKG_VERSION").to_owned(),
        })
        .await?
        .into_inner();
    info!(
        protocol_version = capabilities.protocol_version.as_str(),
        "completed RTP capability exchange"
    );

    Ok(PluginClient {
        inner,
        _capabilities: capabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tcp_target_with_hostname_resolves() {
        let target: Target = "tcp://localhost:50051".parse().expect("parse target");
        let Target::Tcp(authority) = target else {
            panic!("expected tcp target");
        };

        resolve_tcp_authority(&authority)
            .await
            .expect("localhost should resolve");
    }

    #[test]
    fn target_parse_tcp() {
        let target: Target = "tcp://127.0.0.1:8080".parse().unwrap();
        assert!(matches!(target, Target::Tcp(ref a) if a == "127.0.0.1:8080"));
    }

    #[test]
    fn target_parse_http() {
        let target: Target = "http://example.com:50051".parse().unwrap();
        assert!(matches!(target, Target::Tcp(ref a) if a == "example.com:50051"));
    }

    #[test]
    fn target_parse_unix() {
        // The http::Uri parser requires a dummy authority after "unix://".
        let target: Target = "unix://_/var/run/plugin.sock".parse().unwrap();
        assert!(
            matches!(target, Target::Unix(ref p) if p.to_str() == Some("/var/run/plugin.sock"))
        );
    }

    #[test]
    fn target_parse_missing_scheme() {
        let err = "no-scheme".parse::<Target>().unwrap_err();
        assert!(matches!(err, TargetParseError::MissingScheme));
    }

    #[test]
    fn target_parse_unsupported_scheme() {
        let err = "ftp://host:21".parse::<Target>().unwrap_err();
        assert!(matches!(err, TargetParseError::UnsupportedScheme(_)));
    }

    #[test]
    fn validated_resource_id_accepts_valid() {
        let id = validated_resource_id("Std/Random.Int", "seed").unwrap();
        assert_eq!(id.typ, "Std/Random.Int");
        assert_eq!(id.name, "seed");
    }

    #[test]
    fn validated_resource_id_rejects_empty_type() {
        assert!(validated_resource_id("", "seed").is_err());
    }

    #[test]
    fn validated_resource_id_rejects_empty_name() {
        assert!(validated_resource_id("Std/Random.Int", "").is_err());
    }

    #[test]
    fn parse_json_field_rejects_oversized_input() {
        let big = "x".repeat(MAX_JSON_FIELD_BYTES + 1);
        let result = parse_json_field::<sclc::Record>(&big, "test", "T", "n", "rpc");
        assert!(result.is_err());
    }

    #[test]
    fn decode_marker_unknown_value_returns_none() {
        assert!(decode_marker(999).is_none());
    }

    #[test]
    fn decode_marker_known_values() {
        assert_eq!(decode_marker(0), Some(sclc::Marker::Volatile));
        assert_eq!(decode_marker(1), Some(sclc::Marker::Sticky));
    }
}

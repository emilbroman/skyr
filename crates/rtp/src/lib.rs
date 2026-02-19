use std::{net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc};

use anyhow::Context;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Server};
use tower::service_fn;

pub mod proto {
    tonic::include_proto!("rtp.v1");
}

pub use proto::{
    CapabilityRequest, CapabilityResponse, CreateResourceRequest, CreateResourceResponse,
    DeleteResourceRequest, HealthRequest, HealthResponse, Resource, UpdateResourceRequest,
    UpdateResourceResponse,
};

type ResourceTransitionPluginClient =
    proto::resource_transition_plugin_client::ResourceTransitionPluginClient<Channel>;

#[tonic::async_trait]
pub trait Plugin: Send + Sync + 'static {
    async fn create_resource(
        &mut self,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource>;

    async fn update_resource(
        &mut self,
        id: sclc::ResourceId,
        prev_inputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource>;

    async fn delete_resource(&mut self, id: sclc::ResourceId) -> anyhow::Result<()> {
        let _ = id;
        Ok(())
    }

    async fn health(
        &self,
        id: sclc::ResourceId,
        resource: sclc::Resource,
    ) -> anyhow::Result<sclc::Resource> {
        let _ = id;
        Ok(resource)
    }
}

#[derive(Debug, Clone)]
pub enum Target {
    Tcp(SocketAddr),
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

    #[error("invalid tcp address: {0}")]
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
                let addr: SocketAddr = authority.as_str().parse().map_err(|_| {
                    TargetParseError::InvalidTcpAddress(authority.as_str().to_owned())
                })?;
                Ok(Target::Tcp(addr))
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
}

#[derive(Debug, Error)]
pub enum DialError {
    #[error("invalid target: {0}")]
    Target(#[from] TargetParseError),

    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("invalid endpoint uri: {0}")]
    InvalidUri(#[from] http::uri::InvalidUri),

    #[error("capability exchange failed: {0}")]
    CapabilityExchange(#[from] tonic::Status),
}

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
        *self.peer_capabilities.write().await = Some(request.into_inner());

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
        let inputs: sclc::Record =
            serde_json::from_str(&request.resource_inputs_json).map_err(|error| {
                tonic::Status::invalid_argument(format!("invalid resource_inputs_json: {error}"))
            })?;
        let resource_id = sclc::ResourceId {
            ty: request.resource_type.clone(),
            id: request.resource_id.clone(),
        };

        let resource = {
            let mut plugin = self.plugin.write().await;
            plugin
                .create_resource(resource_id.clone(), inputs)
                .await
                .map_err(|error| tonic::Status::internal(error.to_string()))?
        };
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
        let prev_inputs: sclc::Record = serde_json::from_str(&request.prev_resource_inputs_json)
            .map_err(|error| {
                tonic::Status::invalid_argument(format!(
                    "invalid prev_resource_inputs_json: {error}"
                ))
            })?;
        let inputs: sclc::Record =
            serde_json::from_str(&request.resource_inputs_json).map_err(|error| {
                tonic::Status::invalid_argument(format!("invalid resource_inputs_json: {error}"))
            })?;
        let resource_id = sclc::ResourceId {
            ty: request.resource_type.clone(),
            id: request.resource_id.clone(),
        };

        let resource = {
            let mut plugin = self.plugin.write().await;
            plugin
                .update_resource(resource_id.clone(), prev_inputs, inputs)
                .await
                .map_err(|error| tonic::Status::internal(error.to_string()))?
        };
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
        let resource_id = sclc::ResourceId {
            ty: request.resource_type,
            id: request.resource_id,
        };

        {
            let mut plugin = self.plugin.write().await;
            plugin
                .delete_resource(resource_id)
                .await
                .map_err(|error| tonic::Status::internal(error.to_string()))?;
        }

        Ok(tonic::Response::new(()))
    }

    async fn health(
        &self,
        request: tonic::Request<HealthRequest>,
    ) -> Result<tonic::Response<HealthResponse>, tonic::Status> {
        self.ensure_peer_capabilities().await?;

        let resource = request
            .into_inner()
            .resource
            .ok_or_else(|| tonic::Status::invalid_argument("missing health resource"))?;
        let id = sclc::ResourceId {
            ty: resource.r#type.clone(),
            id: resource.id.clone(),
        };
        let parsed = decode_resource(resource)?;

        let plugin = self.plugin.read().await;
        let healthy = plugin
            .health(id.clone(), parsed)
            .await
            .map_err(|error| tonic::Status::internal(error.to_string()))?;

        Ok(tonic::Response::new(HealthResponse {
            resource: Some(encode_resource(id, healthy)?),
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
            return Err(tonic::Status::failed_precondition(
                "capability exchange required",
            ));
        }
        Ok(())
    }
}

pub struct PluginClient {
    inner: ResourceTransitionPluginClient,
    _capabilities: CapabilityResponse,
}

impl PluginClient {
    pub async fn create_resource(
        &mut self,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let resource_inputs_json = serde_json::to_string(&inputs)?;
        let response = self
            .inner
            .create_resource(CreateResourceRequest {
                resource_type: id.ty,
                resource_id: id.id,
                resource_inputs_json,
            })
            .await?
            .into_inner();

        let resource = response.resource.context("missing resource in response")?;
        let inputs: sclc::Record = serde_json::from_str(&resource.inputs_json)?;
        let outputs: sclc::Record = serde_json::from_str(&resource.outputs_json)?;

        Ok(sclc::Resource { inputs, outputs })
    }

    pub async fn update_resource(
        &mut self,
        id: sclc::ResourceId,
        prev_inputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let prev_resource_inputs_json = serde_json::to_string(&prev_inputs)?;
        let resource_inputs_json = serde_json::to_string(&inputs)?;
        let response = self
            .inner
            .update_resource(UpdateResourceRequest {
                resource_type: id.ty,
                resource_id: id.id,
                prev_resource_inputs_json,
                resource_inputs_json,
            })
            .await?
            .into_inner();

        let resource = response.resource.context("missing resource in response")?;
        let inputs: sclc::Record = serde_json::from_str(&resource.inputs_json)?;
        let outputs: sclc::Record = serde_json::from_str(&resource.outputs_json)?;

        Ok(sclc::Resource { inputs, outputs })
    }

    pub async fn delete_resource(&mut self, id: sclc::ResourceId) -> anyhow::Result<()> {
        self.inner
            .delete_resource(DeleteResourceRequest {
                resource_type: id.ty,
                resource_id: id.id,
            })
            .await?;
        Ok(())
    }

    pub async fn health(
        &mut self,
        id: sclc::ResourceId,
        resource: sclc::Resource,
    ) -> anyhow::Result<sclc::Resource> {
        let response = self
            .inner
            .health(HealthRequest {
                resource: Some(encode_resource(id, resource)?),
            })
            .await?
            .into_inner();
        decode_resource(
            response
                .resource
                .context("missing resource in health response")
                .map_err(|error| tonic::Status::internal(error.to_string()))?,
        )
        .map_err(Into::into)
    }
}

fn encode_resource(
    id: sclc::ResourceId,
    resource: sclc::Resource,
) -> Result<Resource, tonic::Status> {
    let inputs_json = serde_json::to_string(&resource.inputs)
        .map_err(|error| tonic::Status::internal(error.to_string()))?;
    let outputs_json = serde_json::to_string(&resource.outputs)
        .map_err(|error| tonic::Status::internal(error.to_string()))?;
    Ok(Resource {
        r#type: id.ty,
        id: id.id,
        inputs_json,
        outputs_json,
    })
}

fn decode_resource(resource: Resource) -> Result<sclc::Resource, tonic::Status> {
    let inputs: sclc::Record = serde_json::from_str(&resource.inputs_json)
        .map_err(|error| tonic::Status::invalid_argument(error.to_string()))?;
    let outputs: sclc::Record = serde_json::from_str(&resource.outputs_json)
        .map_err(|error| tonic::Status::invalid_argument(error.to_string()))?;
    Ok(sclc::Resource { inputs, outputs })
}

pub async fn serve<P, F>(target: impl AsRef<str>, plugin_fn: F) -> Result<(), ServeError>
where
    P: Plugin,
    F: Fn() -> P + Send + Sync + 'static,
{
    let target: Target = target.as_ref().parse()?;
    let factory = PluginFactory::<P, F>::new(plugin_fn);
    let service = proto::resource_transition_plugin_server::ResourceTransitionPluginServer::new(
        factory.make_connection_service(),
    );

    match target {
        Target::Tcp(addr) => {
            Server::builder().add_service(service).serve(addr).await?;
        }
        Target::Unix(path) => {
            if path.exists() {
                tokio::fs::remove_file(&path).await?;
            }
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
            let client =
                proto::resource_transition_plugin_client::ResourceTransitionPluginClient::connect(
                    format!("http://{addr}"),
                )
                .await?;
            Ok(client)
        }
        Target::Unix(path) => {
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
            Ok(
                proto::resource_transition_plugin_client::ResourceTransitionPluginClient::new(
                    channel,
                ),
            )
        }
    }
}

pub async fn dial(target: impl AsRef<str>) -> Result<PluginClient, DialError> {
    let mut inner = dial_raw(target.as_ref().parse()?).await?;
    let capabilities = inner
        .exchange_capabilities(CapabilityRequest {
            plugin_name: String::from("rtp"),
            plugin_version: env!("CARGO_PKG_VERSION").to_owned(),
        })
        .await?
        .into_inner();

    Ok(PluginClient {
        inner,
        _capabilities: capabilities,
    })
}

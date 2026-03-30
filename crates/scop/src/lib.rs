//! Skyr Container Orchestrator Protocol (SCOP)
//!
//! This crate defines the protocol between the container plugin
//! and worker node conduits (SCOC). There are two services:
//!
//! ## Orchestrator Service (served by container plugin)
//!
//! Handles node registration. Conduits connect to register themselves.
//!
//! ```ignore
//! use scop::{Orchestrator, serve_orchestrator};
//!
//! struct MyOrchestrator { /* node registry client */ }
//!
//! impl Orchestrator for MyOrchestrator {
//!     async fn register_node(&self, request: RegisterNodeRequest) -> Result<RegisterNodeResponse, Status> {
//!         // Persist node info to registry
//!     }
//! }
//!
//! serve_orchestrator("0.0.0.0:50053", MyOrchestrator::new()).await?;
//! ```
//!
//! ## Conduit Service (served by SCOC)
//!
//! Handles pod and container operations.
//!
//! ```ignore
//! use scop::{Conduit, serve_conduit};
//!
//! struct MyConduit { /* CRI client */ }
//!
//! impl Conduit for MyConduit {
//!     async fn create_pod(&self, request: CreatePodRequest) -> Result<CreatePodResponse, Status> {
//!         // Use CRI to create the sandbox
//!     }
//! }
//!
//! serve_conduit("0.0.0.0:50054", MyConduit::new()).await?;
//! ```
//!
//! ## Client Usage
//!
//! ```ignore
//! // SCOC registering with orchestrator
//! let mut client = OrchestratorClient::connect("http://plugin:50053").await?;
//! client.register_node(request).await?;
//!
//! // Plugin sending commands to conduit
//! let mut client = ConduitClient::connect("http://node:50054").await?;
//! client.create_pod(request).await?;
//! ```
//!
//! ## Transport Security
//!
//! SCOP does not enforce TLS at the protocol level. In production deployments,
//! transport encryption **must** be provided by the infrastructure layer (e.g.,
//! a service mesh, encrypted overlay network, or Unix domain sockets). TCP
//! listeners should never be exposed on untrusted networks without TLS
//! termination in front.

use std::{net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc};

use thiserror::Error;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Server, server::Router};
use tracing::{info, warn};

// Re-export tonic for the async_trait macro
pub use tonic;

pub mod proto {
    tonic::include_proto!("scop.v1");
}

// ---------------------------------------------------------------------------
// Re-exports grouped by domain
// ---------------------------------------------------------------------------

// --- Node registration (Orchestrator) ---
pub use proto::{
    HeartbeatRequest, HeartbeatResponse, NodeCapacity, NodeUsage, RegisterNodeRequest,
    RegisterNodeResponse, UnregisterNodeRequest, UnregisterNodeResponse,
};

// --- Pod lifecycle (Conduit) ---
pub use proto::{
    ContainerConfig, CreatePodRequest, CreatePodResponse, KeyValue, PodConfig, RemovePodRequest,
    RemovePodResponse,
};

// --- Attachment / firewall ---
pub use proto::{
    AddAttachmentRequest, AddAttachmentResponse, ClosePortRequest, ClosePortResponse,
    OpenPortRequest, OpenPortResponse, RemoveAttachmentRequest, RemoveAttachmentResponse,
};

// --- Overlay networking ---
pub use proto::{
    AddOverlayPeerRequest, AddOverlayPeerResponse, RemoveOverlayPeerRequest,
    RemoveOverlayPeerResponse,
};

// --- Service routing & DNS ---
pub use proto::{
    AddServiceRouteRequest, AddServiceRouteResponse, ConfigureServiceCidrRequest,
    ConfigureServiceCidrResponse, RemoveDnsRecordRequest, RemoveDnsRecordResponse,
    RemoveServiceRouteRequest, RemoveServiceRouteResponse, ServiceBackend, SetDnsRecordRequest,
    SetDnsRecordResponse,
};

// --- Port forwarding ---
pub use proto::{
    PortForwardInit, PortForwardRequest, PortForwardResponse,
    port_forward_request::Payload as PortForwardPayload,
};

// Re-export the generated clients
pub use proto::conduit_client::ConduitClient;
pub use proto::orchestrator_client::OrchestratorClient;

// ============================================================================
// Validation helpers
// ============================================================================

/// Validation helpers for protocol message fields.
///
/// These functions return a [`tonic::Status`] with code `InvalidArgument`
/// when validation fails, making them convenient to use inside service
/// implementations with the `?` operator.
pub mod validate {
    use std::net::{IpAddr, Ipv4Addr};
    use tonic::Status;

    /// Maximum length for node names and pod names.
    const MAX_NAME_LEN: usize = 253;

    /// Validate a port number is within the valid TCP/UDP range (1..=65535).
    pub fn port(value: i32, field: &str) -> Result<u16, Status> {
        u16::try_from(value).ok().filter(|&p| p > 0).ok_or_else(|| {
            Status::invalid_argument(format!("{field}: port must be 1..=65535, got {value}"))
        })
    }

    /// Validate that a protocol string is `"tcp"` or `"udp"`.
    pub fn protocol(value: &str, field: &str) -> Result<(), Status> {
        match value {
            "tcp" | "udp" => Ok(()),
            other => Err(Status::invalid_argument(format!(
                "{field}: protocol must be \"tcp\" or \"udp\", got \"{other}\""
            ))),
        }
    }

    /// Validate a CIDR string (e.g. `"10.42.0.0/16"`).
    ///
    /// Checks that the string parses as `<IPv4>/<prefix>` with a prefix
    /// length between 0 and 32.
    pub fn cidr(value: &str, field: &str) -> Result<(Ipv4Addr, u8), Status> {
        let (ip_str, prefix_str) = value.split_once('/').ok_or_else(|| {
            Status::invalid_argument(format!("{field}: expected CIDR notation (IP/prefix)"))
        })?;
        let ip: Ipv4Addr = ip_str.parse().map_err(|_| {
            Status::invalid_argument(format!("{field}: invalid IPv4 address \"{ip_str}\""))
        })?;
        let prefix: u8 = prefix_str.parse().map_err(|_| {
            Status::invalid_argument(format!("{field}: invalid prefix length \"{prefix_str}\""))
        })?;
        if prefix > 32 {
            return Err(Status::invalid_argument(format!(
                "{field}: prefix length must be 0..=32, got {prefix}"
            )));
        }
        Ok((ip, prefix))
    }

    /// Validate an IP address string.
    pub fn ip_address(value: &str, field: &str) -> Result<IpAddr, Status> {
        value.parse::<IpAddr>().map_err(|_| {
            Status::invalid_argument(format!("{field}: invalid IP address \"{value}\""))
        })
    }

    /// Validate a node or pod name.
    ///
    /// Names must be non-empty, at most 253 characters, and contain only
    /// alphanumerics, hyphens, underscores, dots, colons, or slashes.
    pub fn name(value: &str, field: &str) -> Result<(), Status> {
        if value.is_empty() {
            return Err(Status::invalid_argument(format!(
                "{field}: name must not be empty"
            )));
        }
        if value.len() > MAX_NAME_LEN {
            return Err(Status::invalid_argument(format!(
                "{field}: name must be at most {MAX_NAME_LEN} characters"
            )));
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
        {
            return Err(Status::invalid_argument(format!(
                "{field}: name contains invalid characters (allowed: alphanumeric, -, _, ., :, /)"
            )));
        }
        Ok(())
    }

    /// Validate a container image reference.
    ///
    /// Must be non-empty and contain only printable ASCII (no control chars).
    pub fn image(value: &str, field: &str) -> Result<(), Status> {
        if value.is_empty() {
            return Err(Status::invalid_argument(format!(
                "{field}: image must not be empty"
            )));
        }
        if !value.chars().all(|c| c.is_ascii_graphic()) {
            return Err(Status::invalid_argument(format!(
                "{field}: image contains invalid characters"
            )));
        }
        Ok(())
    }

    /// Validate a hostname (e.g. for DNS records).
    ///
    /// Must be non-empty, at most 253 characters, and contain only
    /// alphanumerics, hyphens, and dots.
    pub fn hostname(value: &str, field: &str) -> Result<(), Status> {
        if value.is_empty() {
            return Err(Status::invalid_argument(format!(
                "{field}: hostname must not be empty"
            )));
        }
        if value.len() > MAX_NAME_LEN {
            return Err(Status::invalid_argument(format!(
                "{field}: hostname must be at most {MAX_NAME_LEN} characters"
            )));
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        {
            return Err(Status::invalid_argument(format!(
                "{field}: hostname contains invalid characters (allowed: alphanumeric, -, .)"
            )));
        }
        Ok(())
    }
}

// ============================================================================
// Common Types
// ============================================================================

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

// ============================================================================
// Orchestrator Service (served by container plugin)
// ============================================================================

/// Trait implemented by the container plugin to handle node registration.
#[tonic::async_trait]
pub trait Orchestrator: Send + Sync + 'static {
    /// Register a node with its conduit address and capacity.
    async fn register_node(
        &self,
        request: RegisterNodeRequest,
    ) -> Result<RegisterNodeResponse, tonic::Status>;

    /// Handle a heartbeat from a registered node.
    async fn heartbeat(
        &self,
        request: HeartbeatRequest,
    ) -> Result<HeartbeatResponse, tonic::Status>;

    /// Unregister a node.
    async fn unregister_node(
        &self,
        request: UnregisterNodeRequest,
    ) -> Result<UnregisterNodeResponse, tonic::Status>;
}

#[derive(Clone)]
struct OrchestratorService<O> {
    orchestrator: Arc<O>,
}

#[tonic::async_trait]
impl<O: Orchestrator> proto::orchestrator_server::Orchestrator for OrchestratorService<O> {
    async fn register_node(
        &self,
        request: tonic::Request<RegisterNodeRequest>,
    ) -> Result<tonic::Response<RegisterNodeResponse>, tonic::Status> {
        let response = self
            .orchestrator
            .register_node(request.into_inner())
            .await?;
        Ok(tonic::Response::new(response))
    }

    async fn heartbeat(
        &self,
        request: tonic::Request<HeartbeatRequest>,
    ) -> Result<tonic::Response<HeartbeatResponse>, tonic::Status> {
        let response = self.orchestrator.heartbeat(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn unregister_node(
        &self,
        request: tonic::Request<UnregisterNodeRequest>,
    ) -> Result<tonic::Response<UnregisterNodeResponse>, tonic::Status> {
        let response = self
            .orchestrator
            .unregister_node(request.into_inner())
            .await?;
        Ok(tonic::Response::new(response))
    }
}

/// Clean up a Unix socket path safely, checking for symlinks to prevent
/// symlink-following attacks (TOCTOU-hardened).
async fn cleanup_unix_socket(path: &std::path::Path) -> Result<(), std::io::Error> {
    // Use symlink_metadata (lstat) to inspect the path without following symlinks.
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                // Refuse to remove symlinks — a symlink here could point to an
                // arbitrary file and we must not follow it.
                warn!(
                    path = %path.display(),
                    "refusing to remove Unix socket: path is a symlink"
                );
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Unix socket path is a symlink",
                ));
            }
            tokio::fs::remove_file(path).await?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Path does not exist — nothing to clean up.
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

async fn serve(target: Target, name: &str, router: Router) -> Result<(), ServeError> {
    match target {
        Target::Tcp(addr) => {
            let addr: SocketAddr = addr
                .parse()
                .map_err(|_| ServeError::InvalidTcpBindAddress(addr.clone()))?;
            info!(addr = %addr, "serving {name} over TCP");
            router.serve(addr).await?;
        }
        Target::Unix(path) => {
            cleanup_unix_socket(&path).await?;
            info!(path = %path.display(), "serving {name} over Unix socket");
            let listener = tokio::net::UnixListener::bind(path)?;
            let incoming = UnixListenerStream::new(listener);
            router.serve_with_incoming(incoming).await?;
        }
    }
    Ok(())
}

/// Serve the Orchestrator service (container plugin side).
pub async fn serve_orchestrator<O: Orchestrator>(
    target: impl AsRef<str>,
    orchestrator: O,
) -> Result<(), ServeError> {
    let target: Target = target.as_ref().parse()?;
    info!(target = ?target, "starting Orchestrator server");

    let service = proto::orchestrator_server::OrchestratorServer::new(OrchestratorService {
        orchestrator: Arc::new(orchestrator),
    });

    serve(
        target,
        "Orchestrator",
        Server::builder().add_service(service),
    )
    .await
}

// ============================================================================
// Conduit Service (served by SCOC)
// ============================================================================

/// Trait implemented by SCOC to handle pod and container operations.
#[tonic::async_trait]
pub trait Conduit: Send + Sync + 'static {
    /// Create a pod with all its containers.
    async fn create_pod(
        &self,
        request: CreatePodRequest,
    ) -> Result<CreatePodResponse, tonic::Status>;

    /// Remove a pod and all its containers.
    async fn remove_pod(
        &self,
        request: RemovePodRequest,
    ) -> Result<RemovePodResponse, tonic::Status>;

    /// Add an egress attachment (open egress port on pod firewall).
    async fn add_attachment(
        &self,
        request: AddAttachmentRequest,
    ) -> Result<AddAttachmentResponse, tonic::Status>;

    /// Remove an egress attachment (close egress port on pod firewall).
    async fn remove_attachment(
        &self,
        request: RemoveAttachmentRequest,
    ) -> Result<RemoveAttachmentResponse, tonic::Status>;

    /// Add a VXLAN overlay peer for cross-node pod communication.
    async fn add_overlay_peer(
        &self,
        request: AddOverlayPeerRequest,
    ) -> Result<AddOverlayPeerResponse, tonic::Status>;

    /// Remove a VXLAN overlay peer.
    async fn remove_overlay_peer(
        &self,
        request: RemoveOverlayPeerRequest,
    ) -> Result<RemoveOverlayPeerResponse, tonic::Status>;

    /// Open an ingress firewall port on a pod.
    async fn open_port(&self, request: OpenPortRequest) -> Result<OpenPortResponse, tonic::Status>;

    /// Close an ingress firewall port on a pod.
    async fn close_port(
        &self,
        request: ClosePortRequest,
    ) -> Result<ClosePortResponse, tonic::Status>;

    /// Add a DNAT service route for Host.Port load balancing.
    async fn add_service_route(
        &self,
        request: AddServiceRouteRequest,
    ) -> Result<AddServiceRouteResponse, tonic::Status>;

    /// Remove a DNAT service route.
    async fn remove_service_route(
        &self,
        request: RemoveServiceRouteRequest,
    ) -> Result<RemoveServiceRouteResponse, tonic::Status>;

    /// Set a DNS record (hostname -> VIP).
    async fn set_dns_record(
        &self,
        request: SetDnsRecordRequest,
    ) -> Result<SetDnsRecordResponse, tonic::Status>;

    /// Remove a DNS record.
    async fn remove_dns_record(
        &self,
        request: RemoveDnsRecordRequest,
    ) -> Result<RemoveDnsRecordResponse, tonic::Status>;

    /// Configure the service CIDR for VIP routing.
    async fn configure_service_cidr(
        &self,
        request: ConfigureServiceCidrRequest,
    ) -> Result<ConfigureServiceCidrResponse, tonic::Status>;

    /// Establish a bidirectional TCP tunnel to a pod's port.
    ///
    /// The first message on the request stream must be a `PortForwardInit`
    /// identifying the target pod and port. Subsequent messages carry raw TCP
    /// data in each direction.
    async fn port_forward(
        &self,
        request: tonic::Streaming<PortForwardRequest>,
    ) -> Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<PortForwardResponse, tonic::Status>> + Send>,
        >,
        tonic::Status,
    >;
}

#[derive(Clone)]
struct ConduitService<C> {
    conduit: Arc<C>,
}

#[tonic::async_trait]
impl<C: Conduit> proto::conduit_server::Conduit for ConduitService<C> {
    async fn create_pod(
        &self,
        request: tonic::Request<CreatePodRequest>,
    ) -> Result<tonic::Response<CreatePodResponse>, tonic::Status> {
        let response = self.conduit.create_pod(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn remove_pod(
        &self,
        request: tonic::Request<RemovePodRequest>,
    ) -> Result<tonic::Response<RemovePodResponse>, tonic::Status> {
        let response = self.conduit.remove_pod(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn add_attachment(
        &self,
        request: tonic::Request<AddAttachmentRequest>,
    ) -> Result<tonic::Response<AddAttachmentResponse>, tonic::Status> {
        let response = self.conduit.add_attachment(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn remove_attachment(
        &self,
        request: tonic::Request<RemoveAttachmentRequest>,
    ) -> Result<tonic::Response<RemoveAttachmentResponse>, tonic::Status> {
        let response = self.conduit.remove_attachment(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn add_overlay_peer(
        &self,
        request: tonic::Request<AddOverlayPeerRequest>,
    ) -> Result<tonic::Response<AddOverlayPeerResponse>, tonic::Status> {
        let response = self.conduit.add_overlay_peer(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn remove_overlay_peer(
        &self,
        request: tonic::Request<RemoveOverlayPeerRequest>,
    ) -> Result<tonic::Response<RemoveOverlayPeerResponse>, tonic::Status> {
        let response = self
            .conduit
            .remove_overlay_peer(request.into_inner())
            .await?;
        Ok(tonic::Response::new(response))
    }

    async fn open_port(
        &self,
        request: tonic::Request<OpenPortRequest>,
    ) -> Result<tonic::Response<OpenPortResponse>, tonic::Status> {
        let response = self.conduit.open_port(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn close_port(
        &self,
        request: tonic::Request<ClosePortRequest>,
    ) -> Result<tonic::Response<ClosePortResponse>, tonic::Status> {
        let response = self.conduit.close_port(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn add_service_route(
        &self,
        request: tonic::Request<AddServiceRouteRequest>,
    ) -> Result<tonic::Response<AddServiceRouteResponse>, tonic::Status> {
        let response = self.conduit.add_service_route(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn remove_service_route(
        &self,
        request: tonic::Request<RemoveServiceRouteRequest>,
    ) -> Result<tonic::Response<RemoveServiceRouteResponse>, tonic::Status> {
        let response = self
            .conduit
            .remove_service_route(request.into_inner())
            .await?;
        Ok(tonic::Response::new(response))
    }

    async fn set_dns_record(
        &self,
        request: tonic::Request<SetDnsRecordRequest>,
    ) -> Result<tonic::Response<SetDnsRecordResponse>, tonic::Status> {
        let response = self.conduit.set_dns_record(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn remove_dns_record(
        &self,
        request: tonic::Request<RemoveDnsRecordRequest>,
    ) -> Result<tonic::Response<RemoveDnsRecordResponse>, tonic::Status> {
        let response = self.conduit.remove_dns_record(request.into_inner()).await?;
        Ok(tonic::Response::new(response))
    }

    async fn configure_service_cidr(
        &self,
        request: tonic::Request<ConfigureServiceCidrRequest>,
    ) -> Result<tonic::Response<ConfigureServiceCidrResponse>, tonic::Status> {
        let response = self
            .conduit
            .configure_service_cidr(request.into_inner())
            .await?;
        Ok(tonic::Response::new(response))
    }

    type PortForwardStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<PortForwardResponse, tonic::Status>> + Send>,
    >;

    async fn port_forward(
        &self,
        request: tonic::Request<tonic::Streaming<PortForwardRequest>>,
    ) -> Result<tonic::Response<Self::PortForwardStream>, tonic::Status> {
        let stream = self.conduit.port_forward(request.into_inner()).await?;
        Ok(tonic::Response::new(stream))
    }
}

/// Serve the Conduit service (SCOC side).
pub async fn serve_conduit<C: Conduit>(
    target: impl AsRef<str>,
    conduit: C,
) -> Result<(), ServeError> {
    let target: Target = target.as_ref().parse()?;
    info!(target = ?target, "starting Conduit server");

    let service = proto::conduit_server::ConduitServer::new(ConduitService {
        conduit: Arc::new(conduit),
    });

    serve(target, "Conduit", Server::builder().add_service(service)).await
}

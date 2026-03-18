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

use std::{net::SocketAddr, path::PathBuf, str::FromStr, sync::Arc};

use thiserror::Error;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Server, server::Router};
use tracing::info;

// Re-export tonic for the async_trait macro
pub use tonic;

pub mod proto {
    tonic::include_proto!("scop.v1");
}

// Re-export commonly used types
pub use proto::{
    AddAttachmentRequest, AddAttachmentResponse, AddOverlayPeerRequest, AddOverlayPeerResponse,
    AddServiceRouteRequest, AddServiceRouteResponse, ClosePortRequest, ClosePortResponse,
    ConfigureServiceCidrRequest, ConfigureServiceCidrResponse, ContainerConfig, CreatePodRequest,
    CreatePodResponse, HeartbeatRequest, HeartbeatResponse, KeyValue, NodeCapacity, NodeUsage,
    OpenPortRequest, OpenPortResponse, PodConfig, RegisterNodeRequest, RegisterNodeResponse,
    RemoveAttachmentRequest, RemoveAttachmentResponse, RemoveDnsRecordRequest,
    RemoveDnsRecordResponse, RemoveOverlayPeerRequest, RemoveOverlayPeerResponse, RemovePodRequest,
    RemovePodResponse, RemoveServiceRouteRequest, RemoveServiceRouteResponse, ServiceBackend,
    SetDnsRecordRequest, SetDnsRecordResponse, UnregisterNodeRequest, UnregisterNodeResponse,
};

// Re-export the generated clients
pub use proto::conduit_client::ConduitClient;
pub use proto::orchestrator_client::OrchestratorClient;

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
            if path.exists() {
                tokio::fs::remove_file(&path).await?;
            }
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

    /// Set a DNS record (hostname → VIP).
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

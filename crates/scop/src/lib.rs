//! Skyr Container Orchestrator Protocol (SCOP)
//!
//! This crate defines the protocol between the container plugin
//! and worker node agents (SCOC).
//!
//! The protocol uses bidirectional gRPC streaming:
//! - Agents (SCOC) connect to the plugin and send a registration message
//! - The plugin sends commands to agents and receives responses
//!
//! # Agent (SCOC) Usage
//!
//! ```ignore
//! use scop::{Agent, connect};
//!
//! struct MyAgent { /* CRI client, etc */ }
//!
//! impl Agent for MyAgent {
//!     async fn run_pod(&mut self, config: PodConfig) -> Result<String> {
//!         // Use CRI to create the sandbox
//!     }
//!     // ... other methods
//! }
//!
//! let agent = MyAgent::new();
//! connect("http://plugin:50053", "node-1", agent).await?;
//! ```
//!
//! # Plugin Usage
//!
//! ```ignore
//! use scop::{Orchestrator, serve, NodeHandle};
//!
//! struct MyOrchestrator { /* node registry, etc */ }
//!
//! impl Orchestrator for MyOrchestrator {
//!     async fn on_node_registered(&mut self, node: NodeHandle) {
//!         // Store the node handle for later use
//!     }
//! }
//!
//! serve("0.0.0.0:50053", MyOrchestrator::new()).await?;
//! ```

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    pin::Pin,
    str::FromStr,
    sync::Arc,
};

use futures::StreamExt;
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tonic::transport::{Channel, Endpoint, Server};
use tower::service_fn;
use tracing::{debug, error, info, warn};

// Re-export tonic for the async_trait macro
pub use tonic;

pub mod proto {
    tonic::include_proto!("scop.v1");
}

pub use proto::{
    CommandError, CommandResponse, CommandSuccess, ContainerConfig, ContainerMetadata,
    CreateContainerRequest, CreateContainerResponse, KeyValue, NodeRegistration,
    PodConfig, PodMetadata, RemoveContainerRequest, RemoveContainerResponse,
    RemovePodRequest, RemovePodResponse, RunPodRequest,
    RunPodResponse, StartContainerRequest, StartContainerResponse, StopContainerRequest,
    StopContainerResponse, StopPodRequest, StopPodResponse,
};

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

// ============================================================================
// Agent (SCOC) Side
// ============================================================================

/// Trait implemented by the agent (SCOC) to handle commands from the plugin.
#[tonic::async_trait]
pub trait Agent: Send + 'static {
    /// Run a pod.
    async fn run_pod(
        &mut self,
        config: PodConfig,
    ) -> anyhow::Result<RunPodResponse>;

    /// Stop a pod.
    async fn stop_pod(&mut self, pod_id: String) -> anyhow::Result<()>;

    /// Remove a pod.
    async fn remove_pod(&mut self, pod_id: String) -> anyhow::Result<()>;

    /// Create a container.
    async fn create_container(
        &mut self,
        pod_id: String,
        config: ContainerConfig,
        pod_config: PodConfig,
    ) -> anyhow::Result<CreateContainerResponse>;

    /// Start a container.
    async fn start_container(&mut self, container_id: String) -> anyhow::Result<()>;

    /// Stop a container.
    async fn stop_container(&mut self, container_id: String, timeout: i64) -> anyhow::Result<()>;

    /// Remove a container.
    async fn remove_container(&mut self, container_id: String) -> anyhow::Result<()>;
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("connection error: {0}")]
    Connection(#[from] tonic::Status),

    #[error("registration rejected: {0}")]
    RegistrationRejected(String),

    #[error("unexpected message from plugin")]
    UnexpectedMessage,

    #[error("stream closed unexpectedly")]
    StreamClosed,
}

/// Connect to the plugin and run the agent loop.
///
/// Note: This function is named `dial` to avoid conflict with the
/// generated gRPC client method.
///
/// This function blocks until the connection is closed or an error occurs.
pub async fn dial<A: Agent>(
    target: impl AsRef<str>,
    node_name: impl Into<String>,
    labels: HashMap<String, String>,
    mut agent: A,
) -> Result<(), ConnectError> {
    let target: Target = target
        .as_ref()
        .parse()
        .map_err(|e| tonic::Status::invalid_argument(format!("invalid target: {e}")))?;
    let node_name = node_name.into();

    info!(target = ?&target, node_name = %node_name, "connecting to SCOP plugin");

    let channel = match target {
        Target::Tcp(addr) => {
            debug!(addr = %addr, "dialing SCOP over TCP");
            Channel::from_shared(format!("http://{addr}"))
                .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?
                .connect()
                .await?
        }
        Target::Unix(path) => {
            debug!(path = %path.display(), "dialing SCOP over Unix socket");
            let endpoint = Endpoint::try_from("http://[::]:50053")
                .map_err(|e| tonic::Status::internal(e.to_string()))?;
            endpoint
                .connect_with_connector(service_fn(move |_| {
                    let path = path.clone();
                    async move {
                        tokio::net::UnixStream::connect(path)
                            .await
                            .map(TokioIo::new)
                    }
                }))
                .await?
        }
    };

    let mut client = proto::container_orchestrator_client::ContainerOrchestratorClient::new(channel);

    // Set up the bidirectional stream
    let (tx, rx) = mpsc::channel::<proto::AgentMessage>(32);
    let request_stream = ReceiverStream::new(rx);

    let mut response_stream = client.session(request_stream).await?.into_inner();

    // Send registration
    info!(node_name = %node_name, "sending node registration");
    tx.send(proto::AgentMessage {
        message: Some(proto::agent_message::Message::Registration(
            NodeRegistration {
                node_name: node_name.clone(),
                labels,
            },
        )),
    })
    .await
    .map_err(|_| ConnectError::StreamClosed)?;

    // Wait for registration ack
    let ack = response_stream
        .next()
        .await
        .ok_or(ConnectError::StreamClosed)?
        .map_err(ConnectError::Connection)?;

    match ack.message {
        Some(proto::plugin_message::Message::RegistrationAck(ack)) => {
            if !ack.success {
                return Err(ConnectError::RegistrationRejected(ack.error));
            }
            info!(node_name = %node_name, "node registration successful");
        }
        _ => return Err(ConnectError::UnexpectedMessage),
    }

    // Process commands
    while let Some(msg) = response_stream.next().await {
        let msg = msg.map_err(ConnectError::Connection)?;

        match msg.message {
            Some(proto::plugin_message::Message::Command(cmd)) => {
                let request_id = cmd.request_id.clone();
                debug!(request_id = %request_id, "received command");

                let result = process_command(&mut agent, cmd).await;

                let response = proto::AgentMessage {
                    message: Some(proto::agent_message::Message::Response(CommandResponse {
                        request_id,
                        result: Some(result),
                    })),
                };

                if tx.send(response).await.is_err() {
                    warn!("failed to send response, stream closed");
                    break;
                }
            }
            Some(proto::plugin_message::Message::RegistrationAck(_)) => {
                warn!("received unexpected registration ack");
            }
            None => {}
        }
    }

    info!(node_name = %node_name, "SCOP connection closed");
    Ok(())
}

async fn process_command<A: Agent>(
    agent: &mut A,
    cmd: proto::Command,
) -> proto::command_response::Result {
    let success = match cmd.command {
        Some(proto::command::Command::RunPod(req)) => {
            let config = req.config.unwrap_or_default();
            match agent.run_pod(config).await {
                Ok(resp) => proto::command_success::Result::RunPod(resp),
                Err(e) => {
                    error!(err = %e, "run_pod failed");
                    return proto::command_response::Result::Error(CommandError {
                        message: e.to_string(),
                    });
                }
            }
        }
        Some(proto::command::Command::StopPod(req)) => {
            match agent.stop_pod(req.pod_id).await {
                Ok(()) => {
                    proto::command_success::Result::StopPod(StopPodResponse {})
                }
                Err(e) => {
                    error!(err = %e, "stop_pod failed");
                    return proto::command_response::Result::Error(CommandError {
                        message: e.to_string(),
                    });
                }
            }
        }
        Some(proto::command::Command::RemovePod(req)) => {
            match agent.remove_pod(req.pod_id).await {
                Ok(()) => {
                    proto::command_success::Result::RemovePod(RemovePodResponse {})
                }
                Err(e) => {
                    error!(err = %e, "remove_pod failed");
                    return proto::command_response::Result::Error(CommandError {
                        message: e.to_string(),
                    });
                }
            }
        }
        Some(proto::command::Command::CreateContainer(req)) => {
            let config = req.config.unwrap_or_default();
            let pod_config = req.pod_config.unwrap_or_default();
            match agent
                .create_container(req.pod_id, config, pod_config)
                .await
            {
                Ok(resp) => proto::command_success::Result::CreateContainer(resp),
                Err(e) => {
                    error!(err = %e, "create_container failed");
                    return proto::command_response::Result::Error(CommandError {
                        message: e.to_string(),
                    });
                }
            }
        }
        Some(proto::command::Command::StartContainer(req)) => {
            match agent.start_container(req.container_id).await {
                Ok(()) => {
                    proto::command_success::Result::StartContainer(StartContainerResponse {})
                }
                Err(e) => {
                    error!(err = %e, "start_container failed");
                    return proto::command_response::Result::Error(CommandError {
                        message: e.to_string(),
                    });
                }
            }
        }
        Some(proto::command::Command::StopContainer(req)) => {
            match agent.stop_container(req.container_id, req.timeout).await {
                Ok(()) => proto::command_success::Result::StopContainer(StopContainerResponse {}),
                Err(e) => {
                    error!(err = %e, "stop_container failed");
                    return proto::command_response::Result::Error(CommandError {
                        message: e.to_string(),
                    });
                }
            }
        }
        Some(proto::command::Command::RemoveContainer(req)) => {
            match agent.remove_container(req.container_id).await {
                Ok(()) => {
                    proto::command_success::Result::RemoveContainer(RemoveContainerResponse {})
                }
                Err(e) => {
                    error!(err = %e, "remove_container failed");
                    return proto::command_response::Result::Error(CommandError {
                        message: e.to_string(),
                    });
                }
            }
        }
        None => {
            return proto::command_response::Result::Error(CommandError {
                message: "empty command".to_string(),
            });
        }
    };

    proto::command_response::Result::Success(CommandSuccess {
        result: Some(success),
    })
}

// ============================================================================
// Plugin Side
// ============================================================================

/// Handle to a connected node, used by the plugin to send commands.
#[derive(Clone)]
pub struct NodeHandle {
    pub node_name: String,
    pub labels: HashMap<String, String>,
    tx: mpsc::Sender<proto::PluginMessage>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<CommandSuccess, String>>>>>,
}

impl NodeHandle {
    /// Run a pod on this node.
    pub async fn run_pod(
        &self,
        config: PodConfig,
    ) -> Result<RunPodResponse, NodeCommandError> {
        let success = self
            .send_command(proto::command::Command::RunPod(
                RunPodRequest {
                    config: Some(config),
                },
            ))
            .await?;

        match success.result {
            Some(proto::command_success::Result::RunPod(resp)) => Ok(resp),
            _ => Err(NodeCommandError::UnexpectedResponse),
        }
    }

    /// Stop a pod on this node.
    pub async fn stop_pod(&self, pod_id: String) -> Result<(), NodeCommandError> {
        let success = self
            .send_command(proto::command::Command::StopPod(
                StopPodRequest { pod_id },
            ))
            .await?;

        match success.result {
            Some(proto::command_success::Result::StopPod(_)) => Ok(()),
            _ => Err(NodeCommandError::UnexpectedResponse),
        }
    }

    /// Remove a pod on this node.
    pub async fn remove_pod(&self, pod_id: String) -> Result<(), NodeCommandError> {
        let success = self
            .send_command(proto::command::Command::RemovePod(
                RemovePodRequest { pod_id },
            ))
            .await?;

        match success.result {
            Some(proto::command_success::Result::RemovePod(_)) => Ok(()),
            _ => Err(NodeCommandError::UnexpectedResponse),
        }
    }

    /// Create a container on this node.
    pub async fn create_container(
        &self,
        pod_id: String,
        config: ContainerConfig,
        pod_config: PodConfig,
    ) -> Result<CreateContainerResponse, NodeCommandError> {
        let success = self
            .send_command(proto::command::Command::CreateContainer(
                CreateContainerRequest {
                    pod_id,
                    config: Some(config),
                    pod_config: Some(pod_config),
                },
            ))
            .await?;

        match success.result {
            Some(proto::command_success::Result::CreateContainer(resp)) => Ok(resp),
            _ => Err(NodeCommandError::UnexpectedResponse),
        }
    }

    /// Start a container on this node.
    pub async fn start_container(&self, container_id: String) -> Result<(), NodeCommandError> {
        let success = self
            .send_command(proto::command::Command::StartContainer(
                StartContainerRequest { container_id },
            ))
            .await?;

        match success.result {
            Some(proto::command_success::Result::StartContainer(_)) => Ok(()),
            _ => Err(NodeCommandError::UnexpectedResponse),
        }
    }

    /// Stop a container on this node.
    pub async fn stop_container(
        &self,
        container_id: String,
        timeout: i64,
    ) -> Result<(), NodeCommandError> {
        let success = self
            .send_command(proto::command::Command::StopContainer(StopContainerRequest {
                container_id,
                timeout,
            }))
            .await?;

        match success.result {
            Some(proto::command_success::Result::StopContainer(_)) => Ok(()),
            _ => Err(NodeCommandError::UnexpectedResponse),
        }
    }

    /// Remove a container on this node.
    pub async fn remove_container(&self, container_id: String) -> Result<(), NodeCommandError> {
        let success = self
            .send_command(proto::command::Command::RemoveContainer(
                RemoveContainerRequest { container_id },
            ))
            .await?;

        match success.result {
            Some(proto::command_success::Result::RemoveContainer(_)) => Ok(()),
            _ => Err(NodeCommandError::UnexpectedResponse),
        }
    }

    async fn send_command(
        &self,
        command: proto::command::Command,
    ) -> Result<CommandSuccess, NodeCommandError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (response_tx, response_rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            pending.insert(request_id.clone(), response_tx);
        }

        self.tx
            .send(proto::PluginMessage {
                message: Some(proto::plugin_message::Message::Command(proto::Command {
                    request_id: request_id.clone(),
                    command: Some(command),
                })),
            })
            .await
            .map_err(|_| NodeCommandError::Disconnected)?;

        let result = response_rx.await.map_err(|_| NodeCommandError::Disconnected)?;

        result.map_err(NodeCommandError::CommandFailed)
    }
}

#[derive(Debug, Error)]
pub enum NodeCommandError {
    #[error("node disconnected")]
    Disconnected,

    #[error("command failed: {0}")]
    CommandFailed(String),

    #[error("unexpected response type")]
    UnexpectedResponse,
}

/// Trait implemented by the plugin to handle node connections.
#[tonic::async_trait]
pub trait Orchestrator: Send + Sync + 'static {
    /// Called when a node registers. Return true to accept, false to reject.
    async fn on_node_registered(&self, handle: NodeHandle) -> bool;

    /// Called when a node disconnects.
    async fn on_node_disconnected(&self, node_name: &str);
}

struct OrchestratorService<O: Orchestrator> {
    orchestrator: Arc<O>,
}

impl<O: Orchestrator> Clone for OrchestratorService<O> {
    fn clone(&self) -> Self {
        Self {
            orchestrator: Arc::clone(&self.orchestrator),
        }
    }
}

type SessionStream = Pin<
    Box<
        dyn futures::Stream<Item = Result<proto::PluginMessage, tonic::Status>>
            + Send
            + Sync
            + 'static,
    >,
>;

#[tonic::async_trait]
impl<O: Orchestrator> proto::container_orchestrator_server::ContainerOrchestrator
    for OrchestratorService<O>
{
    type SessionStream = SessionStream;

    async fn session(
        &self,
        request: tonic::Request<tonic::Streaming<proto::AgentMessage>>,
    ) -> Result<tonic::Response<Self::SessionStream>, tonic::Status> {
        let mut inbound = request.into_inner();

        // Wait for registration message
        let registration = match inbound.next().await {
            Some(Ok(msg)) => match msg.message {
                Some(proto::agent_message::Message::Registration(reg)) => reg,
                _ => {
                    return Err(tonic::Status::invalid_argument(
                        "first message must be registration",
                    ))
                }
            },
            Some(Err(e)) => return Err(e),
            None => {
                return Err(tonic::Status::invalid_argument(
                    "connection closed before registration",
                ))
            }
        };

        let node_name = registration.node_name.clone();
        info!(node_name = %node_name, "node connecting");

        // Set up outbound channel
        let (tx, rx) = mpsc::channel::<proto::PluginMessage>(32);
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<CommandSuccess, String>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let handle = NodeHandle {
            node_name: node_name.clone(),
            labels: registration.labels,
            tx: tx.clone(),
            pending: Arc::clone(&pending),
        };

        // Check if registration is accepted
        let accepted = self.orchestrator.on_node_registered(handle).await;

        // Send registration ack
        let ack = proto::PluginMessage {
            message: Some(proto::plugin_message::Message::RegistrationAck(
                proto::RegistrationAck {
                    success: accepted,
                    error: if accepted {
                        String::new()
                    } else {
                        "registration rejected".to_string()
                    },
                },
            )),
        };

        if tx.send(ack).await.is_err() {
            return Err(tonic::Status::internal("failed to send ack"));
        }

        if !accepted {
            // Stream will close after ack
            let stream = ReceiverStream::new(rx);
            return Ok(tonic::Response::new(Box::pin(stream.map(Ok)) as SessionStream));
        }

        info!(node_name = %node_name, "node registered");

        // Spawn task to process incoming responses
        let orchestrator = Arc::clone(&self.orchestrator);
        let node_name_clone = node_name.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound.next().await {
                match msg {
                    Ok(msg) => {
                        if let Some(proto::agent_message::Message::Response(resp)) = msg.message {
                            let mut pending_guard = pending.lock().await;
                            if let Some(tx) = pending_guard.remove(&resp.request_id) {
                                let result = match resp.result {
                                    Some(proto::command_response::Result::Success(s)) => Ok(s),
                                    Some(proto::command_response::Result::Error(e)) => {
                                        Err(e.message)
                                    }
                                    None => Err("empty response".to_string()),
                                };
                                let _ = tx.send(result);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(node_name = %node_name_clone, err = %e, "error receiving from node");
                        break;
                    }
                }
            }
            info!(node_name = %node_name_clone, "node disconnected");
            orchestrator.on_node_disconnected(&node_name_clone).await;
        });

        let stream = ReceiverStream::new(rx);
        Ok(tonic::Response::new(
            Box::pin(stream.map(Ok)) as SessionStream
        ))
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

/// Serve the SCOP server, accepting connections from agents.
pub async fn serve<O: Orchestrator>(
    target: impl AsRef<str>,
    orchestrator: O,
) -> Result<(), ServeError> {
    let target: Target = target.as_ref().parse()?;
    info!(target = ?&target, "starting SCOP server");

    let service = proto::container_orchestrator_server::ContainerOrchestratorServer::new(
        OrchestratorService {
            orchestrator: Arc::new(orchestrator),
        },
    );

    match target {
        Target::Tcp(addr) => {
            let addr: SocketAddr = addr
                .parse()
                .map_err(|_| ServeError::InvalidTcpBindAddress(addr.clone()))?;
            info!(addr = %addr, "serving SCOP over TCP");
            Server::builder().add_service(service).serve(addr).await?;
        }
        Target::Unix(path) => {
            if path.exists() {
                tokio::fs::remove_file(&path).await?;
            }
            info!(path = %path.display(), "serving SCOP over Unix socket");
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

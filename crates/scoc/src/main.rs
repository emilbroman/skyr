use std::collections::HashMap;
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use ipnet::Ipv4Net;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

mod cri;
mod dns;
mod log_stream;
mod net;

use cri::CriClient;

#[derive(Parser)]
#[command(name = "scoc")]
#[command(about = "Skyr Container Orchestrator Conduit")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the SCOC conduit daemon.
    Daemon {
        /// Unique name for this node.
        #[arg(long)]
        node_name: String,
        /// Address to bind the Conduit server to.
        #[arg(long, default_value = "0.0.0.0:50054")]
        bind: String,
        /// External address for the plugin to connect to (e.g., "http://node-1:50054").
        #[arg(long)]
        conduit_address: String,
        /// Orchestrator address (container plugin).
        #[arg(long)]
        orchestrator_address: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
        /// CPU capacity in millicores (e.g., 4000 = 4 cores).
        #[arg(long, default_value = "4000")]
        cpu_millis: i64,
        /// Memory capacity in bytes.
        #[arg(long, default_value = "8589934592")]
        memory_bytes: i64,
        /// Maximum number of pods.
        #[arg(long, default_value = "100")]
        max_pods: i32,
        /// LDB broker address for container log streaming.
        #[arg(long, default_value = "127.0.0.1:9092")]
        ldb_brokers: String,
        /// Requested pod subnet size (e.g., "24" or "/24" for a /24 subnet).
        /// Sent to the orchestrator during registration; a larger number means
        /// a smaller subnet (fewer pods). Default /24 = 254 pods.
        #[arg(long, default_value = "24")]
        pod_netmask: String,
    },
    /// Check CRI connectivity and version.
    Version {
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Pod operations for testing.
    Pod {
        #[command(subcommand)]
        action: PodAction,
    },
    /// Container operations for testing.
    Container {
        #[command(subcommand)]
        action: ContainerAction,
    },
}

#[derive(Subcommand)]
enum PodAction {
    /// Create a test pod.
    Create {
        /// Pod name.
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Stop and remove a pod.
    Remove {
        /// Pod ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
}

#[derive(Subcommand)]
enum ContainerAction {
    /// Create a container in a pod.
    Create {
        /// Pod ID.
        #[arg(long)]
        pod_id: String,
        /// Container name.
        #[arg(long)]
        name: String,
        /// Container image.
        #[arg(long)]
        image: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Start a container.
    Start {
        /// Container ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Stop a container.
    Stop {
        /// Container ID.
        #[arg(long)]
        id: String,
        /// Timeout in seconds.
        #[arg(long, default_value = "10")]
        timeout: i64,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
    /// Remove a container.
    Remove {
        /// Container ID.
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "/run/containerd/containerd.sock")]
        containerd_socket: String,
    },
}

/// Tracked pod info for log streaming and network teardown.
struct PodInfo {
    environment_qid: String,
    name: String,
    /// Allocated IP address for the pod.
    #[allow(dead_code)]
    ip: Ipv4Addr,
    /// Path to the pod's network namespace (for port opening/closing).
    netns_path: String,
}

/// Tracked container info for log streaming lifecycle.
struct ContainerInfo {
    pod_id: String,
    name: String,
    /// Resource ID in "ResourceType:ResourceName" format, used to construct
    /// a ResourceQid for LDB log namespacing.
    resource_id: String,
}

/// SCOP Conduit implementation backed by CRI, with per-pod networking.
struct CriConduit {
    cri: Arc<Mutex<CriClient>>,
    ldb_publisher: Option<ldb::Publisher>,
    log_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pods: Arc<Mutex<HashMap<String, PodInfo>>>,
    containers: Arc<Mutex<HashMap<String, ContainerInfo>>>,
    /// Per-node IP address allocator.
    ipam: Arc<Mutex<net::Ipam>>,
    /// The node's pod subnet (for network setup).
    pod_cidr: Ipv4Net,
    /// The cluster-wide CIDR (for egress allow-list rules).
    cluster_cidr: Option<String>,
    /// The service CIDR for Host VIPs (for egress allow-list rules).
    service_cidr: Option<String>,
    /// DNS servers to configure in pods.
    dns_servers: Vec<String>,
    /// Shared DNS records for the internal DNS server.
    dns_records: dns::DnsRecords,
}

impl CriConduit {
    #[allow(clippy::too_many_arguments)]
    fn new(
        cri: CriClient,
        ldb_publisher: Option<ldb::Publisher>,
        ipam: net::Ipam,
        pod_cidr: Ipv4Net,
        cluster_cidr: Option<String>,
        service_cidr: Option<String>,
        dns_servers: Vec<String>,
        dns_records: dns::DnsRecords,
    ) -> Self {
        Self {
            cri: Arc::new(Mutex::new(cri)),
            ldb_publisher,
            log_tasks: Arc::new(Mutex::new(HashMap::new())),
            pods: Arc::new(Mutex::new(HashMap::new())),
            containers: Arc::new(Mutex::new(HashMap::new())),
            ipam: Arc::new(Mutex::new(ipam)),
            pod_cidr,
            cluster_cidr,
            service_cidr,
            dns_servers,
            dns_records,
        }
    }

    /// Convert SCOP PodConfig to CRI PodSandboxConfig.
    fn to_cri_pod_config(&self, config: &scop::PodConfig) -> k8s_cri::v1::PodSandboxConfig {
        cri::pod_sandbox_config(&config.name, &self.dns_servers)
    }

    /// Convert SCOP ContainerConfig to CRI ContainerConfig.
    fn to_cri_container_config(config: &scop::ContainerConfig) -> k8s_cri::v1::ContainerConfig {
        let mut cri_config = cri::container_config(&config.name, &config.image);

        if !config.command.is_empty() {
            cri_config.command = config.command.clone();
        }

        if !config.args.is_empty() {
            cri_config.args = config.args.clone();
        }

        if !config.envs.is_empty() {
            cri_config.envs = config
                .envs
                .iter()
                .map(|kv| k8s_cri::v1::KeyValue {
                    key: kv.key.clone(),
                    value: kv.value.clone(),
                })
                .collect();
        }

        cri_config
    }

    /// Start a log streaming task for a container.
    async fn start_log_streaming(&self, container_id: &str) {
        let publisher = match &self.ldb_publisher {
            Some(p) => p.clone(),
            None => return,
        };

        let (pod_name, container_name, ldb_namespace) = {
            let containers = self.containers.lock().await;
            let Some(container_info) = containers.get(container_id) else {
                tracing::warn!(
                    container_id = %container_id,
                    "no container info found for log streaming"
                );
                return;
            };
            let pods = self.pods.lock().await;
            let Some(pod_info) = pods.get(&container_info.pod_id) else {
                tracing::warn!(
                    container_id = %container_id,
                    pod_id = %container_info.pod_id,
                    "no pod info found for log streaming"
                );
                return;
            };

            // Build the LDB namespace from the resource QID. The plugin sends the
            // resource ID in "ResourceType:ResourceName" format; combined with the
            // environment QID this forms a full ResourceQid.
            let namespace = if container_info.resource_id.is_empty() {
                // Fallback for older plugins that don't send the resource ID yet.
                format!(
                    "{}::{}",
                    pod_info.environment_qid, container_info.name,
                )
            } else {
                match (
                    pod_info.environment_qid.parse::<ids::EnvironmentQid>(),
                    container_info.resource_id.parse::<ids::ResourceId>(),
                ) {
                    (Ok(env_qid), Ok(resource_id)) => {
                        ids::ResourceQid::new(env_qid, resource_id).to_string()
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        tracing::warn!(
                            container_id = %container_id,
                            environment_qid = %pod_info.environment_qid,
                            resource_id = %container_info.resource_id,
                            error = %e,
                            "invalid environment QID or resource ID for log namespace, using raw string"
                        );
                        format!(
                            "{}::{}",
                            pod_info.environment_qid, container_info.resource_id,
                        )
                    }
                }
            };

            (
                pod_info.name.clone(),
                container_info.name.clone(),
                namespace,
            )
        };

        let namespace_publisher = match publisher.namespace(ldb_namespace.clone()).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    container_id = %container_id,
                    ldb_namespace = %ldb_namespace,
                    error = %e,
                    "failed to create LDB namespace publisher for container logs"
                );
                return;
            }
        };

        // Log path: /var/log/pods/skyr_{pod_name}/{container_name}/0.log
        let log_path = PathBuf::from(format!(
            "/var/log/pods/skyr_{pod_name}/{container_name}/0.log"
        ));

        let cancel = CancellationToken::new();
        {
            let mut tasks = self.log_tasks.lock().await;
            tasks.insert(container_id.to_string(), cancel.clone());
        }

        tracing::info!(
            container_id = %container_id,
            ldb_namespace = %ldb_namespace,
            log_path = %log_path.display(),
            "starting container log streaming"
        );

        tokio::spawn(async move {
            log_stream::stream_container_logs(log_path, namespace_publisher, cancel).await;
        });
    }

    /// Cancel a log streaming task for a container.
    async fn cancel_log_streaming(&self, container_id: &str) {
        let mut tasks = self.log_tasks.lock().await;
        if let Some(cancel) = tasks.remove(container_id) {
            tracing::info!(
                container_id = %container_id,
                "cancelling container log streaming"
            );
            cancel.cancel();
        }
    }
}

#[scop::tonic::async_trait]
impl scop::Conduit for CriConduit {
    async fn create_pod(
        &self,
        request: scop::CreatePodRequest,
    ) -> Result<scop::CreatePodResponse, scop::tonic::Status> {
        let config = request.config.unwrap_or_default();
        let cri_config = self.to_cri_pod_config(&config);
        let mut cri = self.cri.lock().await;

        // Step 1: Create the CRI pod sandbox (gets its own network namespace)
        let pod_id = cri
            .run_pod_sandbox(cri_config)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Step 2: Allocate an IP for this pod
        let ip = {
            let mut ipam = self.ipam.lock().await;
            match ipam.allocate(&pod_id) {
                Ok(ip) => ip,
                Err(e) => {
                    // Clean up CRI sandbox on IPAM failure
                    let _ = cri.stop_pod_sandbox(&pod_id).await;
                    let _ = cri.remove_pod_sandbox(&pod_id).await;
                    return Err(scop::tonic::Status::internal(format!(
                        "IPAM allocation failed: {e}"
                    )));
                }
            }
        };

        // Step 3: Discover the pod's network namespace path
        let netns_path = match cri.pod_network_namespace(&pod_id).await {
            Ok(path) => path,
            Err(e) => {
                let mut ipam = self.ipam.lock().await;
                ipam.release(&pod_id);
                let _ = cri.stop_pod_sandbox(&pod_id).await;
                let _ = cri.remove_pod_sandbox(&pod_id).await;
                return Err(scop::tonic::Status::internal(format!(
                    "failed to get network namespace: {e}"
                )));
            }
        };

        // Step 4: Set up pod networking (veth pair, bridge, IP, firewall, egress rules)
        if let Err(e) = net::setup_pod_network(
            &pod_id,
            ip,
            &self.pod_cidr,
            &netns_path,
            &config.allowed_destinations,
            self.cluster_cidr.as_deref(),
            self.service_cidr.as_deref(),
        ) {
            let mut ipam = self.ipam.lock().await;
            ipam.release(&pod_id);
            let _ = cri.stop_pod_sandbox(&pod_id).await;
            let _ = cri.remove_pod_sandbox(&pod_id).await;
            return Err(scop::tonic::Status::internal(format!(
                "pod network setup failed: {e:#}"
            )));
        }

        let address = ip.to_string();

        // Track pod info for log streaming and network teardown
        {
            let mut pods = self.pods.lock().await;
            pods.insert(
                pod_id.clone(),
                PodInfo {
                    environment_qid: config.environment_qid,
                    name: config.name,
                    ip,
                    netns_path,
                },
            );
        }

        Ok(scop::CreatePodResponse { pod_id, address })
    }

    async fn remove_pod(
        &self,
        request: scop::RemovePodRequest,
    ) -> Result<scop::RemovePodResponse, scop::tonic::Status> {
        // Tear down pod networking before stopping the sandbox
        // (the netns disappears when the sandbox process exits)
        if let Err(e) = net::teardown_pod_network(&request.pod_id) {
            tracing::warn!(
                pod_id = %request.pod_id,
                error = %e,
                "failed to tear down pod network (continuing with removal)"
            );
        }

        // Release the pod's IP
        {
            let mut ipam = self.ipam.lock().await;
            ipam.release(&request.pod_id);
        }

        let mut cri = self.cri.lock().await;

        // Stop the pod sandbox first, then remove it
        cri.stop_pod_sandbox(&request.pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        cri.remove_pod_sandbox(&request.pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Clean up pod info
        {
            let mut pods = self.pods.lock().await;
            pods.remove(&request.pod_id);
        }

        Ok(scop::RemovePodResponse {})
    }

    async fn create_container(
        &self,
        request: scop::CreateContainerRequest,
    ) -> Result<scop::CreateContainerResponse, scop::tonic::Status> {
        let config = request.config.unwrap_or_default();
        let pod_config = request.pod_config.unwrap_or_default();
        let cri_pod_config = self.to_cri_pod_config(&pod_config);
        let cri_container_config = Self::to_cri_container_config(&config);
        let mut cri = self.cri.lock().await;

        // Pull the image first to ensure it's available in the CRI namespace
        cri.pull_image(&config.image, None)
            .await
            .map_err(|e| scop::tonic::Status::internal(format!("failed to pull image: {}", e)))?;

        let container_id = cri
            .create_container(&request.pod_id, &cri_pod_config, cri_container_config)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Track container info for log streaming
        {
            let mut containers = self.containers.lock().await;
            containers.insert(
                container_id.clone(),
                ContainerInfo {
                    pod_id: request.pod_id,
                    name: config.name,
                    resource_id: request.resource_id,
                },
            );
        }

        Ok(scop::CreateContainerResponse { container_id })
    }

    async fn start_container(
        &self,
        request: scop::StartContainerRequest,
    ) -> Result<scop::StartContainerResponse, scop::tonic::Status> {
        let mut cri = self.cri.lock().await;
        cri.start_container(&request.container_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        drop(cri);

        // Start log streaming after container starts
        self.start_log_streaming(&request.container_id).await;

        Ok(scop::StartContainerResponse {})
    }

    async fn stop_container(
        &self,
        request: scop::StopContainerRequest,
    ) -> Result<scop::StopContainerResponse, scop::tonic::Status> {
        // Cancel log streaming before stopping
        self.cancel_log_streaming(&request.container_id).await;

        let mut cri = self.cri.lock().await;
        cri.stop_container(&request.container_id, request.timeout)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        Ok(scop::StopContainerResponse {})
    }

    async fn remove_container(
        &self,
        request: scop::RemoveContainerRequest,
    ) -> Result<scop::RemoveContainerResponse, scop::tonic::Status> {
        // Cancel log streaming if still running
        self.cancel_log_streaming(&request.container_id).await;

        let mut cri = self.cri.lock().await;
        cri.remove_container(&request.container_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Clean up container info
        {
            let mut containers = self.containers.lock().await;
            containers.remove(&request.container_id);
        }

        Ok(scop::RemoveContainerResponse {})
    }

    async fn add_overlay_peer(
        &self,
        request: scop::AddOverlayPeerRequest,
    ) -> Result<scop::AddOverlayPeerResponse, scop::tonic::Status> {
        // Resolve hostname to IP (bridge fdb requires IP address)
        let peer_ip = resolve_hostname_to_ip(&request.peer_host_ip).map_err(|e| {
            scop::tonic::Status::internal(format!(
                "failed to resolve peer {}: {e:#}",
                request.peer_host_ip
            ))
        })?;
        net::add_overlay_peer(&peer_ip).map_err(|e| {
            scop::tonic::Status::internal(format!("add overlay peer failed: {e:#}"))
        })?;
        Ok(scop::AddOverlayPeerResponse {})
    }

    async fn remove_overlay_peer(
        &self,
        request: scop::RemoveOverlayPeerRequest,
    ) -> Result<scop::RemoveOverlayPeerResponse, scop::tonic::Status> {
        // Resolve hostname to IP (bridge fdb requires IP address)
        let peer_ip = resolve_hostname_to_ip(&request.peer_host_ip).map_err(|e| {
            scop::tonic::Status::internal(format!(
                "failed to resolve peer {}: {e:#}",
                request.peer_host_ip
            ))
        })?;
        net::remove_overlay_peer(&peer_ip).map_err(|e| {
            scop::tonic::Status::internal(format!("remove overlay peer failed: {e:#}"))
        })?;
        Ok(scop::RemoveOverlayPeerResponse {})
    }

    async fn open_port(
        &self,
        request: scop::OpenPortRequest,
    ) -> Result<scop::OpenPortResponse, scop::tonic::Status> {
        let pods = self.pods.lock().await;
        let pod = pods.get(&request.pod_id).ok_or_else(|| {
            scop::tonic::Status::not_found(format!("pod not found: {}", request.pod_id))
        })?;
        net::open_port(&pod.netns_path, request.port, &request.protocol)
            .map_err(|e| scop::tonic::Status::internal(format!("open port failed: {e:#}")))?;
        Ok(scop::OpenPortResponse {})
    }

    async fn close_port(
        &self,
        request: scop::ClosePortRequest,
    ) -> Result<scop::ClosePortResponse, scop::tonic::Status> {
        let pods = self.pods.lock().await;
        let Some(pod) = pods.get(&request.pod_id) else {
            // Pod already gone — treat as success (idempotent)
            tracing::warn!(
                pod_id = %request.pod_id,
                "close_port: pod not found, treating as success"
            );
            return Ok(scop::ClosePortResponse {});
        };
        net::close_port(&pod.netns_path, request.port, &request.protocol)
            .map_err(|e| scop::tonic::Status::internal(format!("close port failed: {e:#}")))?;
        Ok(scop::ClosePortResponse {})
    }

    async fn add_service_route(
        &self,
        request: scop::AddServiceRouteRequest,
    ) -> Result<scop::AddServiceRouteResponse, scop::tonic::Status> {
        let svc_cidr = self.service_cidr.as_deref().unwrap_or("");
        net::add_service_route(
            &request.vip,
            request.port,
            &request.protocol,
            &request.backends,
            svc_cidr,
        )
        .map_err(|e| scop::tonic::Status::internal(format!("add service route failed: {e:#}")))?;
        Ok(scop::AddServiceRouteResponse {})
    }

    async fn remove_service_route(
        &self,
        request: scop::RemoveServiceRouteRequest,
    ) -> Result<scop::RemoveServiceRouteResponse, scop::tonic::Status> {
        net::remove_service_route(&request.vip, request.port, &request.protocol).map_err(|e| {
            scop::tonic::Status::internal(format!("remove service route failed: {e:#}"))
        })?;
        Ok(scop::RemoveServiceRouteResponse {})
    }

    async fn set_dns_record(
        &self,
        request: scop::SetDnsRecordRequest,
    ) -> Result<scop::SetDnsRecordResponse, scop::tonic::Status> {
        let ip: std::net::Ipv4Addr = request
            .address
            .parse()
            .map_err(|e| scop::tonic::Status::invalid_argument(format!("invalid IP: {e}")))?;

        let mut records = self
            .dns_records
            .write()
            .map_err(|e| scop::tonic::Status::internal(format!("DNS lock poisoned: {e}")))?;
        records.insert(request.hostname.clone(), ip);
        tracing::info!(
            hostname = %request.hostname,
            address = %request.address,
            "DNS record set"
        );
        Ok(scop::SetDnsRecordResponse {})
    }

    async fn remove_dns_record(
        &self,
        request: scop::RemoveDnsRecordRequest,
    ) -> Result<scop::RemoveDnsRecordResponse, scop::tonic::Status> {
        let mut records = self
            .dns_records
            .write()
            .map_err(|e| scop::tonic::Status::internal(format!("DNS lock poisoned: {e}")))?;
        records.remove(&request.hostname);
        tracing::info!(
            hostname = %request.hostname,
            "DNS record removed"
        );
        Ok(scop::RemoveDnsRecordResponse {})
    }

    async fn configure_service_cidr(
        &self,
        request: scop::ConfigureServiceCidrRequest,
    ) -> Result<scop::ConfigureServiceCidrResponse, scop::tonic::Status> {
        net::configure_service_cidr_forwarding(&request.service_cidr).map_err(|e| {
            scop::tonic::Status::internal(format!("configure service CIDR failed: {e:#}"))
        })?;
        Ok(scop::ConfigureServiceCidrResponse {})
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();

    match args.command {
        Command::Daemon {
            node_name,
            bind,
            conduit_address,
            orchestrator_address,
            containerd_socket,
            cpu_millis,
            memory_bytes,
            max_pods,
            ldb_brokers,
            pod_netmask,
        } => {
            // Parse --pod-netmask, stripping optional leading slash
            let pod_netmask: u32 = pod_netmask
                .strip_prefix('/')
                .unwrap_or(&pod_netmask)
                .parse()
                .expect("invalid --pod-netmask, expected a number like 24 or /24");
            assert!(
                pod_netmask > 0 && pod_netmask <= 30,
                "--pod-netmask must be between 1 and 30"
            );

            tracing::info!("SCOC conduit starting");
            tracing::info!("  node_name: {}", node_name);
            tracing::info!("  bind: {}", bind);
            tracing::info!("  conduit_address: {}", conduit_address);
            tracing::info!("  orchestrator_address: {}", orchestrator_address);
            tracing::info!("  containerd_socket: {}", containerd_socket);
            tracing::info!("  ldb_brokers: {}", ldb_brokers);
            tracing::info!("  pod_netmask: /{}", pod_netmask);

            // Verify CRI connectivity at startup
            let cri = {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                let version = cri.version().await?;
                tracing::info!("containerd version: {}", version);
                cri
            };

            // Connect to LDB for container log streaming
            let ldb_publisher = match ldb::ClientBuilder::new()
                .brokers(ldb_brokers)
                .build_publisher()
                .await
            {
                Ok(publisher) => {
                    tracing::info!("Connected to LDB for container log streaming");
                    Some(publisher)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to connect to LDB, container log streaming disabled: {}",
                        e
                    );
                    None
                }
            };

            // Connect to orchestrator and register (with retries)
            tracing::info!("Registering with orchestrator at {}", orchestrator_address);
            let mut retries = 0;
            let max_retries = 30;
            let register_response = loop {
                match scop::OrchestratorClient::connect(orchestrator_address.clone()).await {
                    Ok(mut orchestrator) => {
                        match orchestrator
                            .register_node(scop::RegisterNodeRequest {
                                node_name: node_name.clone(),
                                conduit_address: conduit_address.clone(),
                                capacity: Some(scop::NodeCapacity {
                                    cpu_millis,
                                    memory_bytes,
                                    max_pods,
                                }),
                                labels: Default::default(),
                                pod_netmask,
                            })
                            .await
                        {
                            Ok(response) => break response.into_inner(),
                            Err(e) => {
                                retries += 1;
                                if retries >= max_retries {
                                    anyhow::bail!(
                                        "Failed to register with orchestrator after {} retries: {}",
                                        max_retries,
                                        e
                                    );
                                }
                                tracing::warn!(
                                    "Registration failed (attempt {}/{}): {}, retrying...",
                                    retries,
                                    max_retries,
                                    e
                                );
                                tokio::time::sleep(Duration::from_secs(2)).await;
                            }
                        }
                    }
                    Err(e) => {
                        retries += 1;
                        if retries >= max_retries {
                            anyhow::bail!(
                                "Failed to connect to orchestrator after {} retries: {}",
                                max_retries,
                                e
                            );
                        }
                        tracing::warn!(
                            "Connection to orchestrator failed (attempt {}/{}): {}, retrying...",
                            retries,
                            max_retries,
                            e
                        );
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            };

            if !register_response.success {
                anyhow::bail!(
                    "Failed to register with orchestrator: {}",
                    register_response.error
                );
            }

            // Parse the pod CIDR assigned by the orchestrator
            let pod_cidr: Ipv4Net = register_response
                .pod_cidr
                .parse()
                .expect("orchestrator returned invalid pod_cidr");
            tracing::info!(
                "Registered with orchestrator, assigned pod CIDR: {}",
                pod_cidr
            );

            // Parse the cluster CIDR for egress allow-list enforcement
            let cluster_cidr = if register_response.cluster_cidr.is_empty() {
                tracing::warn!(
                    "orchestrator did not provide cluster_cidr, egress allow-list enforcement disabled"
                );
                None
            } else {
                tracing::info!(
                    "cluster CIDR for egress rules: {}",
                    register_response.cluster_cidr
                );
                Some(register_response.cluster_cidr)
            };

            // Parse the service CIDR (for Host VIP routing)
            let service_cidr = if register_response.service_cidr.is_empty() {
                tracing::warn!(
                    "orchestrator did not provide service_cidr, Host VIP routing disabled"
                );
                None
            } else {
                tracing::info!(
                    "service CIDR for Host VIPs: {}",
                    register_response.service_cidr
                );
                Some(register_response.service_cidr)
            };

            // Set up the pod bridge network with the assigned CIDR
            net::setup_bridge(&pod_cidr)?;

            // Set up VXLAN overlay for cross-node pod communication
            let local_host = extract_host_from_address(&conduit_address);
            let local_ip = resolve_hostname_to_ip(&local_host)?;
            net::setup_vxlan(&local_ip)?;

            // Set up the DNAT services chain for Host.Port load balancing
            net::setup_services_chain()?;

            // Configure service CIDR forwarding if available
            if let Some(ref svc_cidr) = service_cidr
                && let Err(e) = net::configure_service_cidr_forwarding(svc_cidr)
            {
                tracing::warn!("failed to configure service CIDR forwarding: {e:#}");
            }

            // Set up the internal DNS server for *.internal resolution
            let dns_records = dns::new_records();
            let gateway = std::net::Ipv4Addr::from(u32::from(pod_cidr.network()) + 1);
            let dns_bind_addr = std::net::SocketAddr::new(gateway.into(), 53);
            let dns_records_clone = dns_records.clone();
            let upstream_dns = net::host_nameservers();
            tracing::info!("Starting internal DNS server on {}", dns_bind_addr);
            std::thread::spawn(move || {
                if let Err(e) = dns::run_dns_server(dns_bind_addr, dns_records_clone, upstream_dns)
                {
                    tracing::error!("DNS server error: {e:#}");
                }
            });

            let ipam = net::Ipam::new(pod_cidr);
            // Configure pods to use the bridge gateway as their DNS server
            // (where our internal DNS resolver runs)
            let dns_servers = vec![gateway.to_string()];
            tracing::info!("DNS servers for pods: {:?}", dns_servers);

            // Create the conduit with networking support
            let conduit = CriConduit::new(
                cri,
                ldb_publisher,
                ipam,
                pod_cidr,
                cluster_cidr,
                service_cidr,
                dns_servers,
                dns_records,
            );

            // Spawn heartbeat task
            let node_name_heartbeat = node_name.clone();
            let orchestrator_address_heartbeat = orchestrator_address.clone();
            let heartbeat_handle = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;

                    match scop::OrchestratorClient::connect(orchestrator_address_heartbeat.clone())
                        .await
                    {
                        Ok(mut client) => {
                            if let Err(e) = client
                                .heartbeat(scop::HeartbeatRequest {
                                    node_name: node_name_heartbeat.clone(),
                                    usage: None,
                                })
                                .await
                            {
                                tracing::warn!("Heartbeat failed: {}", e);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to connect for heartbeat: {}", e);
                        }
                    }
                }
            });

            // Start Conduit server in a separate task
            let bind_target = format!("http://{}", bind);
            let server_handle =
                tokio::spawn(async move { scop::serve_conduit(&bind_target, conduit).await });

            // Wait for shutdown signal
            tokio::select! {
                result = server_handle => {
                    if let Err(e) = result {
                        tracing::error!("Conduit server error: {}", e);
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Received shutdown signal");
                }
            }

            // Cancel heartbeat task
            heartbeat_handle.abort();

            // Tear down services chain
            let _ = net::teardown_services_chain();

            // Tear down VXLAN overlay
            let _ = net::teardown_vxlan();

            // Tear down pod bridge network
            if let Err(e) = net::teardown_bridge(&pod_cidr) {
                tracing::error!("Failed to tear down bridge: {}", e);
            }

            // Unregister from orchestrator
            tracing::info!("Unregistering from orchestrator");
            if let Ok(mut client) = scop::OrchestratorClient::connect(orchestrator_address).await
                && let Err(e) = client
                    .unregister_node(scop::UnregisterNodeRequest {
                        node_name: node_name.clone(),
                    })
                    .await
            {
                tracing::error!("Failed to unregister: {}", e);
            }
        }

        Command::Version { containerd_socket } => {
            let mut cri = CriClient::connect(&containerd_socket).await?;
            let version = cri.version().await?;
            println!("Runtime version: {version}");
        }

        Command::Pod { action } => match action {
            PodAction::Create {
                name,
                containerd_socket,
            } => {
                // Test CLI: creates a pod sandbox without networking setup.
                // Use the daemon for full networking support.
                let mut cri = CriClient::connect(&containerd_socket).await?;
                let dns_servers = net::host_nameservers();
                let config = cri::pod_sandbox_config(&name, &dns_servers);
                let pod_id = cri.run_pod_sandbox(config).await?;
                println!("{pod_id}");
            }
            PodAction::Remove {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.stop_pod_sandbox(&id).await?;
                cri.remove_pod_sandbox(&id).await?;
                println!("Pod removed");
            }
        },

        Command::Container { action } => match action {
            ContainerAction::Create {
                pod_id,
                name,
                image,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;

                // Pull the image first to ensure it's in the CRI namespace
                cri.pull_image(&image, None).await?;

                // Create a minimal pod config for the container creation call
                let dns_servers = net::host_nameservers();
                let pod_config = cri::pod_sandbox_config("pod", &dns_servers);
                let container_config = cri::container_config(&name, &image);
                let container_id = cri
                    .create_container(&pod_id, &pod_config, container_config)
                    .await?;
                println!("{container_id}");
            }
            ContainerAction::Start {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.start_container(&id).await?;
                println!("Container started");
            }
            ContainerAction::Stop {
                id,
                timeout,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.stop_container(&id, timeout).await?;
                println!("Container stopped");
            }
            ContainerAction::Remove {
                id,
                containerd_socket,
            } => {
                let mut cri = CriClient::connect(&containerd_socket).await?;
                cri.remove_container(&id).await?;
                println!("Container removed");
            }
        },
    }

    Ok(())
}

/// Extract the host from a conduit address like "http://192.168.1.10:50054" or "http://scoc-1:50054".
fn extract_host_from_address(addr: &str) -> String {
    // Strip scheme (e.g., "http://")
    let without_scheme = addr.split("://").nth(1).unwrap_or(addr);
    // Strip path
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    // Strip port
    // Handle IPv6 addresses in brackets: [::1]:port
    if authority.starts_with('[') {
        // IPv6: [host]:port
        authority
            .split(']')
            .next()
            .unwrap_or(authority)
            .trim_start_matches('[')
            .to_owned()
    } else {
        // IPv4 or hostname: host:port
        authority
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority)
            .to_owned()
    }
}

/// Resolve a hostname to an IP address.
///
/// If the input is already an IP address, it is returned as-is.
/// Otherwise, DNS resolution is performed to get the IP.
fn resolve_hostname_to_ip(hostname: &str) -> Result<String> {
    // Add a dummy port for ToSocketAddrs (it requires host:port format)
    let addr_with_port = format!("{}:0", hostname);
    let resolved = addr_with_port
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("could not resolve hostname: {}", hostname))?;
    Ok(resolved.ip().to_string())
}

use std::collections::{HashMap, HashSet};
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

/// Tracked pod info for log streaming and network teardown.
struct PodInfo {
    /// CRI pod sandbox ID (internal to SCOC).
    cri_pod_id: String,
    #[allow(dead_code)]
    environment_qid: String,
    #[allow(dead_code)]
    name: String,
    /// Allocated IP address for the pod.
    #[allow(dead_code)]
    ip: Ipv4Addr,
    /// Path to the pod's network namespace (for port opening/closing).
    netns_path: String,
    /// CRI container IDs for containers within this pod.
    container_ids: Vec<String>,
}

/// Maps Host VIP → set of (port, protocol) for active service routes.
type ServiceRouteMap = HashMap<String, HashSet<(i32, String)>>;

/// SCOP Conduit implementation backed by CRI, with per-pod networking.
struct CriConduit {
    cri: Arc<Mutex<CriClient>>,
    ldb_publisher: Option<ldb::Publisher>,
    log_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Pods keyed by pod_name (full resource name with hash).
    pods: Arc<Mutex<HashMap<String, PodInfo>>>,
    /// Per-node IP address allocator.
    ipam: Arc<Mutex<net::Ipam>>,
    /// The node's pod subnet (for network setup).
    pod_cidr: Ipv4Net,
    /// The cluster-wide CIDR (for egress rules).
    cluster_cidr: Option<String>,
    /// The service CIDR for Host VIPs (for egress rules).
    service_cidr: Option<String>,
    /// DNS servers to configure in pods.
    dns_servers: Vec<String>,
    /// Shared DNS records for the internal DNS server.
    dns_records: dns::DnsRecords,
    /// VIP aliases: maps LAN VIP address → Host VIP (destination).
    /// Used to add SKYR-SERVICES dispatch rules for InternetAddress VIPs.
    vip_aliases: Arc<Mutex<HashMap<String, String>>>,
    /// Active service routes: maps Host VIP → set of (port, protocol).
    /// Tracked so that VIP aliases added after service routes can retroactively
    /// install dispatch rules.
    service_routes: Arc<Mutex<ServiceRouteMap>>,
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
            ipam: Arc::new(Mutex::new(ipam)),
            pod_cidr,
            cluster_cidr,
            service_cidr,
            dns_servers,
            dns_records,
            vip_aliases: Arc::new(Mutex::new(HashMap::new())),
            service_routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Clean up a partially-created pod on failure.
    ///
    /// Stops and removes any containers that were already started, tears down
    /// pod networking, releases the IPAM allocation, and removes the CRI sandbox.
    /// All errors during cleanup are logged but do not propagate.
    async fn cleanup_failed_pod(
        &self,
        cri: &mut CriClient,
        cri_pod_id: &str,
        container_ids: &[String],
        network_setup: bool,
    ) {
        for cid in container_ids {
            let _ = cri.stop_container(cid, 10).await;
            let _ = cri.remove_container(cid).await;
        }
        if network_setup {
            let _ = net::teardown_pod_network(cri_pod_id);
        }
        let mut ipam = self.ipam.lock().await;
        ipam.release(cri_pod_id);
        let _ = cri.stop_pod_sandbox(cri_pod_id).await;
        let _ = cri.remove_pod_sandbox(cri_pod_id).await;
    }

    /// Convert SCOP PodConfig to CRI PodSandboxConfig.
    fn to_cri_pod_config(&self, config: &scop::PodConfig) -> k8s_cri::v1::PodSandboxConfig {
        cri::pod_sandbox_config(&config.name, &config.environment_qid, &self.dns_servers)
    }

    /// Convert SCOP ContainerConfig to CRI ContainerConfig by index.
    fn to_cri_container_config(
        index: usize,
        config: &scop::ContainerConfig,
    ) -> k8s_cri::v1::ContainerConfig {
        let mut cri_config = cri::container_config(index, &config.image);

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

    /// Start a log streaming task for a container within a pod.
    async fn start_log_streaming(
        &self,
        pod_name: &str,
        container_index: usize,
        ldb_namespace: &str,
    ) {
        let publisher = match &self.ldb_publisher {
            Some(p) => p.clone(),
            None => return,
        };

        let namespace_publisher = match publisher.namespace(ldb_namespace.to_string()).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    pod_name = %pod_name,
                    container_index = %container_index,
                    ldb_namespace = %ldb_namespace,
                    error = %e,
                    "failed to create LDB namespace publisher for container logs"
                );
                return;
            }
        };

        // Log path: /var/log/pods/skyr_{pod_name}/{index}/0.log
        let log_path = PathBuf::from(format!(
            "/var/log/pods/skyr_{pod_name}/{container_index}/0.log"
        ));

        let cancel = CancellationToken::new();
        let task_key = format!("{pod_name}_{container_index}");
        {
            let mut tasks = self.log_tasks.lock().await;
            tasks.insert(task_key.clone(), cancel.clone());
        }

        tracing::info!(
            pod_name = %pod_name,
            container_index = %container_index,
            ldb_namespace = %ldb_namespace,
            log_path = %log_path.display(),
            "starting container log streaming"
        );

        tokio::spawn(async move {
            log_stream::stream_container_logs(
                log_path,
                namespace_publisher,
                cancel,
                container_index,
            )
            .await;
        });
    }

    /// Cancel all log streaming tasks for a pod.
    async fn cancel_pod_log_streaming(&self, pod_name: &str, num_containers: usize) {
        let mut tasks = self.log_tasks.lock().await;
        for i in 0..num_containers {
            let task_key = format!("{pod_name}_{i}");
            if let Some(cancel) = tasks.remove(&task_key) {
                tracing::info!(
                    pod_name = %pod_name,
                    container_index = %i,
                    "cancelling container log streaming"
                );
                cancel.cancel();
            }
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
        let pod_name = config.name.clone();
        let cri_pod_config = self.to_cri_pod_config(&config);
        let mut cri = self.cri.lock().await;

        // Step 1: Create the CRI pod sandbox (gets its own network namespace)
        let cri_pod_id = cri
            .run_pod_sandbox(cri_pod_config.clone())
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Step 2: Allocate an IP for this pod
        let ip = {
            let mut ipam = self.ipam.lock().await;
            match ipam.allocate(&cri_pod_id) {
                Ok(ip) => ip,
                Err(e) => {
                    self.cleanup_failed_pod(&mut cri, &cri_pod_id, &[], false)
                        .await;
                    return Err(scop::tonic::Status::internal(format!(
                        "IPAM allocation failed: {e}"
                    )));
                }
            }
        };

        // Step 3: Discover the pod's network namespace path
        let netns_path = match cri.pod_network_namespace(&cri_pod_id).await {
            Ok(path) => path,
            Err(e) => {
                self.cleanup_failed_pod(&mut cri, &cri_pod_id, &[], false)
                    .await;
                return Err(scop::tonic::Status::internal(format!(
                    "failed to get network namespace: {e}"
                )));
            }
        };

        // Step 4: Set up pod networking (veth pair, bridge, IP, firewall, egress rules)
        if let Err(e) = net::setup_pod_network(
            &cri_pod_id,
            ip,
            &self.pod_cidr,
            &netns_path,
            self.cluster_cidr.as_deref(),
            self.service_cidr.as_deref(),
        ) {
            self.cleanup_failed_pod(&mut cri, &cri_pod_id, &[], false)
                .await;
            return Err(scop::tonic::Status::internal(format!(
                "pod network setup failed: {e:#}"
            )));
        }

        // Step 5: Create and start all containers in the pod
        let mut container_ids: Vec<String> = Vec::with_capacity(config.containers.len());
        // Build the LDB namespace from the pod's resource QID
        let resource_id = ids::ResourceId {
            typ: "Std/Container.Pod".to_string(),
            name: pod_name.clone(),
        };
        let ldb_namespace = match config.environment_qid.parse::<ids::EnvironmentQid>() {
            Ok(env_qid) => ids::ResourceQid::new(env_qid, resource_id).to_string(),
            Err(_) => format!("{}::Std/Container.Pod:{}", config.environment_qid, pod_name),
        };

        for (i, container_config) in config.containers.iter().enumerate() {
            let cri_container_config = Self::to_cri_container_config(i, container_config);

            // Pull the image
            if let Err(e) = cri.pull_image(&container_config.image, None).await {
                self.cleanup_failed_pod(&mut cri, &cri_pod_id, &container_ids, true)
                    .await;
                return Err(scop::tonic::Status::internal(format!(
                    "failed to pull image for container {i}: {e}"
                )));
            }

            // Create the container
            let container_id = match cri
                .create_container(&cri_pod_id, &cri_pod_config, cri_container_config)
                .await
            {
                Ok(id) => id,
                Err(e) => {
                    self.cleanup_failed_pod(&mut cri, &cri_pod_id, &container_ids, true)
                        .await;
                    return Err(scop::tonic::Status::internal(format!(
                        "failed to create container {i}: {e}"
                    )));
                }
            };

            // Start the container
            if let Err(e) = cri.start_container(&container_id).await {
                let _ = cri.remove_container(&container_id).await;
                self.cleanup_failed_pod(&mut cri, &cri_pod_id, &container_ids, true)
                    .await;
                return Err(scop::tonic::Status::internal(format!(
                    "failed to start container {i}: {e}"
                )));
            }

            container_ids.push(container_id);
        }

        // Drop CRI lock before starting log streaming
        drop(cri);

        // Step 6: Start log streaming for each container
        for i in 0..container_ids.len() {
            self.start_log_streaming(&pod_name, i, &ldb_namespace).await;
        }

        let address = ip.to_string();

        // Track pod info keyed by pod_name
        {
            let mut pods = self.pods.lock().await;
            pods.insert(
                pod_name,
                PodInfo {
                    cri_pod_id,
                    environment_qid: config.environment_qid,
                    name: config.name,
                    ip,
                    netns_path,
                    container_ids,
                },
            );
        }

        Ok(scop::CreatePodResponse {
            pod_id: String::new(),
            address,
        })
    }

    async fn remove_pod(
        &self,
        request: scop::RemovePodRequest,
    ) -> Result<scop::RemovePodResponse, scop::tonic::Status> {
        let pod_name = &request.pod_name;

        // Look up pod info
        let pod_info = {
            let mut pods = self.pods.lock().await;
            pods.remove(pod_name)
        };

        let Some(pod_info) = pod_info else {
            // Pod already gone — treat as success (idempotent)
            tracing::warn!(
                pod_name = %pod_name,
                "remove_pod: pod not found, treating as success"
            );
            return Ok(scop::RemovePodResponse {});
        };

        // Cancel log streaming for all containers
        self.cancel_pod_log_streaming(pod_name, pod_info.container_ids.len())
            .await;

        let mut cri = self.cri.lock().await;

        // Stop and remove all containers in the pod
        for (i, container_id) in pod_info.container_ids.iter().enumerate() {
            if let Err(e) = cri.stop_container(container_id, 30).await {
                tracing::warn!(
                    pod_name = %pod_name,
                    container_index = %i,
                    container_id = %container_id,
                    error = %e,
                    "failed to stop container (continuing with removal)"
                );
            }
            if let Err(e) = cri.remove_container(container_id).await {
                tracing::warn!(
                    pod_name = %pod_name,
                    container_index = %i,
                    container_id = %container_id,
                    error = %e,
                    "failed to remove container (continuing with removal)"
                );
            }
        }

        // Tear down pod networking before stopping the sandbox
        if let Err(e) = net::teardown_pod_network(&pod_info.cri_pod_id) {
            tracing::warn!(
                pod_name = %pod_name,
                error = %e,
                "failed to tear down pod network (continuing with removal)"
            );
        }

        // Release the pod's IP
        {
            let mut ipam = self.ipam.lock().await;
            ipam.release(&pod_info.cri_pod_id);
        }

        // Stop and remove the CRI sandbox
        cri.stop_pod_sandbox(&pod_info.cri_pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;
        cri.remove_pod_sandbox(&pod_info.cri_pod_id)
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        Ok(scop::RemovePodResponse {})
    }

    async fn add_attachment(
        &self,
        request: scop::AddAttachmentRequest,
    ) -> Result<scop::AddAttachmentResponse, scop::tonic::Status> {
        let pods = self.pods.lock().await;
        let pod = pods.get(&request.pod_name).ok_or_else(|| {
            scop::tonic::Status::not_found(format!("pod not found: {}", request.pod_name))
        })?;
        net::open_egress_port(
            &pod.netns_path,
            &request.destination_address,
            request.port,
            &request.protocol,
        )
        .map_err(|e| scop::tonic::Status::internal(format!("add attachment failed: {e:#}")))?;
        Ok(scop::AddAttachmentResponse {})
    }

    async fn remove_attachment(
        &self,
        request: scop::RemoveAttachmentRequest,
    ) -> Result<scop::RemoveAttachmentResponse, scop::tonic::Status> {
        let pods = self.pods.lock().await;
        let Some(pod) = pods.get(&request.pod_name) else {
            // Pod already gone — treat as success (idempotent)
            tracing::warn!(
                pod_name = %request.pod_name,
                "remove_attachment: pod not found, treating as success"
            );
            return Ok(scop::RemoveAttachmentResponse {});
        };
        net::close_egress_port(
            &pod.netns_path,
            &request.destination_address,
            request.port,
            &request.protocol,
        )
        .map_err(|e| scop::tonic::Status::internal(format!("remove attachment failed: {e:#}")))?;
        Ok(scop::RemoveAttachmentResponse {})
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
        let pod = pods.get(&request.pod_name).ok_or_else(|| {
            scop::tonic::Status::not_found(format!("pod not found: {}", request.pod_name))
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
        let Some(pod) = pods.get(&request.pod_name) else {
            // Pod already gone — treat as success (idempotent)
            tracing::warn!(
                pod_name = %request.pod_name,
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
        .map_err(|e| {
            tracing::error!("add service route failed: {e:#}");
            scop::tonic::Status::internal(format!("add service route failed: {e:#}"))
        })?;

        // Track this service route so future VIP alias additions can install dispatches.
        {
            let mut routes = self.service_routes.lock().await;
            routes
                .entry(request.vip.clone())
                .or_default()
                .insert((request.port, request.protocol.clone()));
        }

        // Add dispatch rules for any existing VIP aliases that point to this VIP.
        {
            let aliases = self.vip_aliases.lock().await;
            for (alias, dest) in aliases.iter() {
                if dest == &request.vip {
                    net::add_vip_dispatch(alias, &request.vip, request.port, &request.protocol)
                        .map_err(|e| {
                            scop::tonic::Status::internal(format!(
                                "add VIP dispatch for alias {alias} failed: {e:#}"
                            ))
                        })?;
                }
            }
        }

        Ok(scop::AddServiceRouteResponse {})
    }

    async fn remove_service_route(
        &self,
        request: scop::RemoveServiceRouteRequest,
    ) -> Result<scop::RemoveServiceRouteResponse, scop::tonic::Status> {
        // Remove dispatch rules for any VIP aliases that point to this VIP.
        {
            let aliases = self.vip_aliases.lock().await;
            for (alias, dest) in aliases.iter() {
                if dest == &request.vip {
                    let _ = net::remove_vip_dispatch(
                        alias,
                        &request.vip,
                        request.port,
                        &request.protocol,
                    );
                }
            }
        }

        net::remove_service_route(&request.vip, request.port, &request.protocol).map_err(|e| {
            scop::tonic::Status::internal(format!("remove service route failed: {e:#}"))
        })?;

        // Remove from tracked service routes.
        {
            let mut routes = self.service_routes.lock().await;
            if let Some(ports) = routes.get_mut(&request.vip) {
                ports.remove(&(request.port, request.protocol.clone()));
                if ports.is_empty() {
                    routes.remove(&request.vip);
                }
            }
        }

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

        // Recover from a poisoned lock rather than permanently failing.
        let mut records = self.dns_records.write().unwrap_or_else(|e| e.into_inner());
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
        // Recover from a poisoned lock rather than permanently failing.
        let mut records = self.dns_records.write().unwrap_or_else(|e| e.into_inner());
        records.remove(&request.hostname);
        tracing::info!(
            hostname = %request.hostname,
            "DNS record removed"
        );
        Ok(scop::RemoveDnsRecordResponse {})
    }

    async fn add_vip(
        &self,
        request: scop::AddVipRequest,
    ) -> Result<scop::AddVipResponse, scop::tonic::Status> {
        scop::validate::ip_address(&request.address, "address")?;
        scop::validate::ip_address(&request.destination, "destination")?;

        // Add the VIP address to the network interface (for ARP/reachability).
        net::add_vip_address(&request.address)
            .map_err(|e| scop::tonic::Status::internal(format!("add VIP failed: {e:#}")))?;

        // Store the alias mapping.
        {
            let mut aliases = self.vip_aliases.lock().await;
            aliases.insert(request.address.clone(), request.destination.clone());
        }

        // Add dispatch rules for any existing service routes targeting the destination.
        {
            let routes = self.service_routes.lock().await;
            if let Some(ports) = routes.get(&request.destination) {
                for (port, protocol) in ports {
                    net::add_vip_dispatch(&request.address, &request.destination, *port, protocol)
                        .map_err(|e| {
                            scop::tonic::Status::internal(format!(
                                "add VIP dispatch for {}:{} failed: {e:#}",
                                request.address, port
                            ))
                        })?;
                }
            }
        }

        Ok(scop::AddVipResponse {})
    }

    async fn remove_vip(
        &self,
        request: scop::RemoveVipRequest,
    ) -> Result<scop::RemoveVipResponse, scop::tonic::Status> {
        scop::validate::ip_address(&request.address, "address")?;

        // Look up the destination from our tracked state.
        let destination = {
            let aliases = self.vip_aliases.lock().await;
            aliases.get(&request.address).cloned()
        };

        // Remove all SKYR-SERVICES dispatch rules for this VIP alias.
        if let Some(dest) = &destination {
            let routes = self.service_routes.lock().await;
            if let Some(ports) = routes.get(dest.as_str()) {
                for (port, protocol) in ports {
                    let _ = net::remove_vip_dispatch(&request.address, dest, *port, protocol);
                }
            }
        }

        // Remove the VIP address from the network interface.
        net::remove_vip_address(&request.address)
            .map_err(|e| scop::tonic::Status::internal(format!("remove VIP failed: {e:#}")))?;

        // Remove from tracked state.
        {
            let mut aliases = self.vip_aliases.lock().await;
            aliases.remove(&request.address);
        }

        Ok(scop::RemoveVipResponse {})
    }

    async fn configure_service_cidr(
        &self,
        _request: scop::ConfigureServiceCidrRequest,
    ) -> Result<scop::ConfigureServiceCidrResponse, scop::tonic::Status> {
        // No-op: service CIDR forwarding is now handled by the bridge FORWARD rule
        // which allows all traffic to the bridge. Pods have their own INPUT firewalls.
        Ok(scop::ConfigureServiceCidrResponse {})
    }

    async fn port_forward(
        &self,
        mut request_stream: scop::tonic::Streaming<scop::PortForwardRequest>,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<Item = Result<scop::PortForwardResponse, scop::tonic::Status>>
                    + Send,
            >,
        >,
        scop::tonic::Status,
    > {
        use futures::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Read the first message which must be an init
        let first = request_stream
            .next()
            .await
            .ok_or_else(|| scop::tonic::Status::invalid_argument("empty request stream"))?
            .map_err(|e| {
                scop::tonic::Status::internal(format!("failed to read init message: {e}"))
            })?;

        let init = match first.payload {
            Some(scop::PortForwardPayload::Init(init)) => init,
            _ => {
                return Err(scop::tonic::Status::invalid_argument(
                    "first message must be PortForwardInit",
                ));
            }
        };

        scop::validate::name(&init.pod_name, "pod_name")?;
        let port = scop::validate::port(init.port, "port")?;

        // Look up the pod to get its IP address
        let pod_ip = {
            let pods = self.pods.lock().await;
            let pod_info = pods.get(&init.pod_name).ok_or_else(|| {
                scop::tonic::Status::not_found(format!("pod not found: {}", init.pod_name))
            })?;
            pod_info.ip
        };

        tracing::info!(
            pod_name = %init.pod_name,
            pod_ip = %pod_ip,
            port = %port,
            "establishing port-forward connection"
        );

        // Connect to the pod's IP and port
        let tcp_stream =
            tokio::net::TcpStream::connect(std::net::SocketAddr::new(pod_ip.into(), port))
                .await
                .map_err(|e| {
                    scop::tonic::Status::unavailable(format!(
                        "failed to connect to {pod_ip}:{port}: {e}"
                    ))
                })?;

        let (mut tcp_read, mut tcp_write) = tcp_stream.into_split();

        // Channel for sending responses back to the client
        let (response_tx, response_rx) = tokio::sync::mpsc::channel::<
            Result<scop::PortForwardResponse, scop::tonic::Status>,
        >(32);

        // Task: read from gRPC request stream → write to TCP socket
        let response_tx_upstream = response_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = request_stream.next().await {
                match msg {
                    Ok(req) => {
                        if let Some(scop::PortForwardPayload::Data(data)) = req.payload
                            && tcp_write.write_all(&data).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tcp_write.shutdown().await;
            drop(response_tx_upstream);
        });

        // Task: read from TCP socket → send on gRPC response stream
        tokio::spawn(async move {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                match tcp_read.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let response = scop::PortForwardResponse {
                            data: buf[..n].to_vec(),
                        };
                        if response_tx.send(Ok(response)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let output_stream = tokio_stream::wrappers::ReceiverStream::new(response_rx);
        Ok(Box::pin(output_stream))
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

            // Spawn heartbeat task with exponential backoff and re-registration
            let node_name_heartbeat = node_name.clone();
            let orchestrator_address_heartbeat = orchestrator_address.clone();
            let conduit_address_heartbeat = conduit_address.clone();
            let pod_cidr_heartbeat = pod_cidr;
            let heartbeat_handle = tokio::spawn(async move {
                const BASE_INTERVAL_SECS: u64 = 30;
                const MAX_BACKOFF_SECS: u64 = 300;
                const RE_REGISTER_THRESHOLD: u32 = 3;

                let mut consecutive_failures: u32 = 0;

                loop {
                    // Calculate sleep duration with exponential backoff on failures
                    let sleep_secs = if consecutive_failures == 0 {
                        BASE_INTERVAL_SECS
                    } else {
                        let backoff =
                            BASE_INTERVAL_SECS * 2u64.saturating_pow(consecutive_failures.min(10));
                        // Add jitter: ±25% to prevent thundering herd
                        let jitter_range = backoff / 4;
                        let jitter = if jitter_range > 0 {
                            // Simple pseudo-random jitter using timestamp nanos
                            let nanos = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .subsec_nanos() as u64;
                            nanos % (jitter_range * 2)
                        } else {
                            0
                        };
                        (backoff - jitter_range + jitter).min(MAX_BACKOFF_SECS)
                    };
                    tokio::time::sleep(Duration::from_secs(sleep_secs)).await;

                    // After too many consecutive failures, attempt re-registration
                    if consecutive_failures >= RE_REGISTER_THRESHOLD {
                        tracing::warn!(
                            consecutive_failures = consecutive_failures,
                            "heartbeat failures exceeded threshold, attempting re-registration"
                        );

                        match scop::OrchestratorClient::connect(
                            orchestrator_address_heartbeat.clone(),
                        )
                        .await
                        {
                            Ok(mut client) => {
                                match client
                                    .register_node(scop::RegisterNodeRequest {
                                        node_name: node_name_heartbeat.clone(),
                                        conduit_address: conduit_address_heartbeat.clone(),
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
                                    Ok(response) => {
                                        let resp = response.into_inner();
                                        if resp.success {
                                            // Verify pod_cidr matches our bridge configuration
                                            if resp.pod_cidr != pod_cidr_heartbeat.to_string() {
                                                tracing::error!(
                                                    expected = %pod_cidr_heartbeat,
                                                    received = %resp.pod_cidr,
                                                    "re-registration returned different pod_cidr"
                                                );
                                            } else {
                                                tracing::info!("re-registration successful");
                                            }
                                            consecutive_failures = 0;
                                            continue;
                                        }
                                        tracing::warn!(
                                            error = %resp.error,
                                            "re-registration failed"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!("re-registration RPC failed: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("failed to connect for re-registration: {}", e);
                            }
                        }
                        consecutive_failures += 1;
                        continue;
                    }

                    // Normal heartbeat
                    match scop::OrchestratorClient::connect(orchestrator_address_heartbeat.clone())
                        .await
                    {
                        Ok(mut client) => {
                            match client
                                .heartbeat(scop::HeartbeatRequest {
                                    node_name: node_name_heartbeat.clone(),
                                    usage: None,
                                })
                                .await
                            {
                                Ok(response) => {
                                    if response.into_inner().acknowledged {
                                        consecutive_failures = 0;
                                    } else {
                                        consecutive_failures += 1;
                                        tracing::warn!(
                                            consecutive_failures = consecutive_failures,
                                            "heartbeat not acknowledged"
                                        );
                                    }
                                }
                                Err(e) => {
                                    consecutive_failures += 1;
                                    tracing::warn!(
                                        consecutive_failures = consecutive_failures,
                                        "heartbeat failed: {}",
                                        e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            tracing::warn!(
                                consecutive_failures = consecutive_failures,
                                "failed to connect for heartbeat: {}",
                                e
                            );
                        }
                    }
                }
            });

            // Start Conduit server in a separate task
            let bind_target = format!("http://{}", bind);
            let server_handle =
                tokio::spawn(async move { scop::serve_conduit(&bind_target, conduit).await });

            // Pull overlay peers from orchestrator after Conduit server has been spawned
            {
                match scop::OrchestratorClient::connect(orchestrator_address.clone()).await {
                    Ok(mut client) => {
                        match client
                            .get_overlay_peers(scop::GetOverlayPeersRequest {
                                node_name: node_name.clone(),
                            })
                            .await
                        {
                            Ok(response) => {
                                let peers = response.into_inner().peers;
                                tracing::info!(
                                    peer_count = peers.len(),
                                    "fetched overlay peers from orchestrator"
                                );
                                for peer in &peers {
                                    let peer_ip = match resolve_hostname_to_ip(&peer.peer_host_ip) {
                                        Ok(ip) => ip,
                                        Err(e) => {
                                            tracing::warn!(peer = %peer.peer_host_ip, error = %e, "failed to resolve overlay peer");
                                            continue;
                                        }
                                    };
                                    if let Err(e) = net::add_overlay_peer(&peer_ip) {
                                        tracing::warn!(peer = %peer.peer_host_ip, error = %e, "failed to add overlay peer");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "failed to fetch overlay peers from orchestrator");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to connect to orchestrator for peer fetch");
                    }
                }
            }

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
                let config = cri::pod_sandbox_config(&name, "test", &dns_servers);
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
///
/// The input is validated to only contain safe characters (alphanumeric, hyphens,
/// dots, colons for IPv6) to prevent injection attacks.
fn resolve_hostname_to_ip(hostname: &str) -> Result<String> {
    anyhow::ensure!(!hostname.is_empty(), "hostname must not be empty");
    anyhow::ensure!(
        hostname
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == ':'),
        "hostname contains invalid characters: {hostname:?}"
    );

    // Add a dummy port for ToSocketAddrs (it requires host:port format)
    let addr_with_port = format!("{hostname}:0");
    let resolved = addr_with_port
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("could not resolve hostname: {hostname}"))?;
    Ok(resolved.ip().to_string())
}

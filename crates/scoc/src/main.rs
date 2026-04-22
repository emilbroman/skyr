use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use ipnet::Ipv4Net;
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;

mod cri;
mod dns;
mod gossip;
mod log_stream;
mod net;

use cri::CriClient;
use gossip::{KnownPeers, MergeEffect};

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
        /// Path to the PEM-encoded CA certificate used to verify the
        /// orchestrator and incoming conduit clients. When set together with
        /// `--tls-cert` and `--tls-key`, the conduit listener and all
        /// orchestrator RPCs use mTLS. All three flags must be provided
        /// together; omit all three to run plain gRPC.
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        /// Path to the PEM-encoded leaf certificate for this node. The cert
        /// must carry both `serverAuth` and `clientAuth` Extended Key Usages.
        #[arg(long)]
        tls_cert: Option<PathBuf>,
        /// Path to the PEM-encoded private key matching `--tls-cert`.
        #[arg(long)]
        tls_key: Option<PathBuf>,
        /// Number of random live peers to push to when a merge produces
        /// net-new information (reactive gossip fan-out).
        #[arg(long, default_value = "3")]
        gossip_fanout: usize,
        /// Interval at which this node initiates an anti-entropy digest
        /// exchange with one random live peer (seconds).
        #[arg(long, default_value = "30")]
        gossip_interval_secs: u64,
        /// How long tombstone records are retained locally after their
        /// `last_seen_micros` before being garbage collected (seconds).
        #[arg(long, default_value = "3600")]
        tombstone_ttl_secs: u64,
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

/// Network state that becomes available only after orchestrator registration.
struct NetworkState {
    ipam: Mutex<net::Ipam>,
    pod_cidr: Ipv4Net,
    cluster_cidr: Option<String>,
    service_cidr: Option<String>,
    dns_servers: Vec<String>,
}

/// Handle used by `main()` to initialize the network state after registration
/// and drain any gossip entries that arrived before initialization.
struct NetworkInitHandle {
    net_state: Arc<OnceCell<NetworkState>>,
    pending_entries: Arc<Mutex<Vec<scop::PeerEntry>>>,
    known_peers: Arc<Mutex<KnownPeers>>,
    self_name: String,
}

impl NetworkInitHandle {
    /// Set the network state and process any buffered overlay gossip.
    async fn initialize(
        self,
        ipam: net::Ipam,
        pod_cidr: Ipv4Net,
        cluster_cidr: Option<String>,
        service_cidr: Option<String>,
        dns_servers: Vec<String>,
    ) -> Result<()> {
        // Hold the pending_entries lock while setting net_state so that
        // gossip_peers cannot observe is_none() == true, get preempted,
        // and then push into the buffer after we have already drained it.
        let entries = {
            let mut pending = self.pending_entries.lock().await;
            self.net_state
                .set(NetworkState {
                    ipam: Mutex::new(ipam),
                    pod_cidr,
                    cluster_cidr,
                    service_cidr,
                    dns_servers,
                })
                .ok()
                .expect("network state already initialized");
            std::mem::take(&mut *pending)
        };

        // Drain buffered gossip entries now that the network is ready.
        let mut known = self.known_peers.lock().await;
        for entry in entries {
            apply_merge_effect(&self.self_name, &mut known, entry)?;
        }

        Ok(())
    }
}

/// Handles passed to background gossip tasks. Small & cheap to clone.
#[derive(Clone)]
struct GossipHandles {
    known_peers: Arc<Mutex<KnownPeers>>,
    self_name: Arc<String>,
    /// Optional TLS material reused across all outbound gossip connections.
    /// Must match the material the peer was started with; `None` means plain
    /// gRPC.
    tls: Option<Arc<scop::TlsMaterial>>,
}

/// Reactive fan-out: pick `fanout` random live peers (excluding self and the
/// sender) and push the just-changed entries to each.
async fn reactive_fanout(handles: GossipHandles, from_node: String, entries: Vec<scop::PeerEntry>) {
    use rand::seq::SliceRandom;

    const DEFAULT_FANOUT: usize = 3;
    let fanout = GOSSIP_FANOUT.load(std::sync::atomic::Ordering::Relaxed);
    let fanout = if fanout == 0 { DEFAULT_FANOUT } else { fanout };

    let targets: Vec<(String, String)> = {
        let known = handles.known_peers.lock().await;
        let mut live = known.live_peers();
        live.retain(|(name, _)| name != handles.self_name.as_str() && name != &from_node);
        live.shuffle(&mut rand::thread_rng());
        live.into_iter().take(fanout).collect()
    };

    for (name, endpoint) in targets {
        let target_name = name.clone();
        let conduit_addr = format!("http://{}:50054", endpoint);
        let from_node = handles.self_name.as_ref().clone();
        let entries = entries.clone();
        let tls = handles.tls.clone();
        tokio::spawn(async move {
            send_gossip_to(
                &conduit_addr,
                &target_name,
                from_node,
                entries,
                None,
                tls.as_deref(),
            )
            .await;
        });
    }
}

/// Background task: periodically pick a random live peer and exchange a
/// digest with them. The response `delta` is merged locally; any newly-
/// accepted entries are reactively fanned out in the merge path itself.
async fn periodic_digest_gossip(
    handles: GossipHandles,
    interval: Duration,
    tls: Option<Arc<scop::TlsMaterial>>,
) {
    use rand::seq::SliceRandom;
    loop {
        tokio::time::sleep(interval).await;

        let (target, digest) = {
            let known = handles.known_peers.lock().await;
            let mut live = known.live_peers();
            live.retain(|(name, _)| name != handles.self_name.as_str());
            let Some(target) = live.choose(&mut rand::thread_rng()).cloned() else {
                continue;
            };
            (target, known.digest())
        };

        let (name, endpoint) = target;
        let conduit_addr = format!("http://{}:50054", endpoint);
        let from_node = handles.self_name.as_ref().clone();

        let result: Result<scop::GossipPeersResponse, String> = async {
            let mut client = scop::connect_conduit(conduit_addr.clone(), tls.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            client
                .gossip_peers(scop::GossipPeersRequest {
                    from_node: from_node.clone(),
                    entries: Vec::new(),
                    digest: Some(digest),
                })
                .await
                .map(|r| r.into_inner())
                .map_err(|e| e.to_string())
        }
        .await;

        let delta = match result {
            Ok(resp) => resp.delta,
            Err(e) => {
                tracing::debug!(peer = %name, error = %e, "digest gossip failed");
                continue;
            }
        };

        if delta.is_empty() {
            continue;
        }

        let self_name = handles.self_name.as_str().to_string();
        let mut known = handles.known_peers.lock().await;
        let mut changed: Vec<scop::PeerEntry> = Vec::new();
        for entry in delta {
            let snapshot = entry.clone();
            if apply_merge_effect(&self_name, &mut known, entry).is_ok()
                && known.iter().any(|(n, s)| {
                    n == &snapshot.node_name
                        && s.last_seen_micros == snapshot.last_seen_micros
                        && s.tombstone == snapshot.tombstone
                })
            {
                changed.push(snapshot);
            }
        }
        drop(known);

        if !changed.is_empty() {
            let handles = handles.clone();
            tokio::spawn(async move {
                reactive_fanout(handles, name, changed).await;
            });
        }
    }
}

/// Background task: periodically GC tombstones whose age exceeds the TTL.
async fn tombstone_gc_task(known_peers: Arc<Mutex<KnownPeers>>, ttl: Duration) {
    // Sweep at half the TTL so tombstones disappear within ~1.5x the TTL on average.
    let interval = std::cmp::max(ttl / 2, Duration::from_secs(60));
    loop {
        tokio::time::sleep(interval).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        let ttl_micros = ttl.as_micros() as u64;
        let pruned = {
            let mut known = known_peers.lock().await;
            known.gc_tombstones(now, ttl_micros)
        };
        if pruned > 0 {
            tracing::debug!(pruned = pruned, "GC'd expired tombstones");
        }
    }
}

async fn send_gossip_to(
    target_address: &str,
    target_name: &str,
    from_node: String,
    entries: Vec<scop::PeerEntry>,
    digest: Option<scop::PeerDigest>,
    tls: Option<&scop::TlsMaterial>,
) {
    match scop::connect_conduit(target_address.to_string(), tls).await {
        Ok(mut client) => {
            if let Err(e) = client
                .gossip_peers(scop::GossipPeersRequest {
                    from_node,
                    entries,
                    digest,
                })
                .await
            {
                tracing::debug!(target = %target_name, error = %e, "gossip push failed");
            }
        }
        Err(e) => {
            tracing::debug!(target = %target_name, error = %e, "failed to connect for gossip");
        }
    }
}

/// Fan-out size configured at startup. Stored as an atomic so background
/// gossip tasks can read it without plumbing it through every call site.
static GOSSIP_FANOUT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Merge one entry and apply the resulting FDB change. Hostname resolution
/// is performed on endpoints that aren't already literal IPs; failures are
/// logged but non-fatal to the merge (the in-memory state still advances so
/// that anti-entropy can re-apply the effect once the name resolves).
fn apply_merge_effect(
    self_name: &str,
    known: &mut KnownPeers,
    entry: scop::PeerEntry,
) -> Result<()> {
    let prior_endpoint = known.existing_endpoint(&entry.node_name);
    let Some(effect) = known.merge_entry(self_name, entry) else {
        return Ok(());
    };
    match effect {
        MergeEffect::AddFdb { overlay_endpoint } => {
            // Endpoint rotation: drop the stale FDB before installing the new one.
            if let Some(old) = prior_endpoint
                && old != overlay_endpoint
                && let Ok(old_ip) = resolve_hostname_to_ip(&old)
            {
                let _ = net::remove_overlay_peer(&old_ip);
            }
            match resolve_hostname_to_ip(&overlay_endpoint)
                .and_then(|ip| net::add_overlay_peer(&ip))
            {
                Ok(()) => tracing::info!(peer = %overlay_endpoint, "added overlay peer via gossip"),
                Err(e) => {
                    tracing::warn!(peer = %overlay_endpoint, error = %e, "failed to add overlay peer")
                }
            }
        }
        MergeEffect::RemoveFdb { overlay_endpoint } => {
            match resolve_hostname_to_ip(&overlay_endpoint)
                .and_then(|ip| net::remove_overlay_peer(&ip))
            {
                Ok(()) => {
                    tracing::info!(peer = %overlay_endpoint, "removed overlay peer via gossip")
                }
                Err(e) => {
                    tracing::warn!(peer = %overlay_endpoint, error = %e, "failed to remove overlay peer")
                }
            }
        }
        MergeEffect::MetadataOnly => {}
    }
    Ok(())
}

/// SCOP Conduit implementation backed by CRI, with per-pod networking.
struct CriConduit {
    cri: Arc<Mutex<CriClient>>,
    ldb_publisher: Option<ldb::Publisher>,
    log_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Pods keyed by pod_name (full resource name with hash).
    pods: Arc<Mutex<HashMap<String, PodInfo>>>,
    /// Network state, set after orchestrator registration via `NetworkInitHandle`.
    net_state: Arc<OnceCell<NetworkState>>,
    /// Gossip entries received before network initialization, buffered so
    /// that FDB updates can be applied once `vxlan1` is ready.
    pending_entries: Arc<Mutex<Vec<scop::PeerEntry>>>,
    /// In-memory membership view driven by Conduit.GossipPeers.
    known_peers: Arc<Mutex<KnownPeers>>,
    /// Canonical hostname of this node (used as `from_node` / `source` in
    /// outbound gossip and to reject entries that name this node).
    self_name: Arc<String>,
    /// Optional TLS material for outbound gossip connections to other
    /// conduits. Matches whatever `--tls-*` flags this node was started
    /// with; `None` means plain gRPC.
    tls: Option<Arc<scop::TlsMaterial>>,
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
    /// Create a `CriConduit` in pending state (before network initialization).
    fn new_pending(
        cri: CriClient,
        ldb_publisher: Option<ldb::Publisher>,
        dns_records: dns::DnsRecords,
        self_name: String,
        tls: Option<Arc<scop::TlsMaterial>>,
    ) -> Self {
        Self {
            cri: Arc::new(Mutex::new(cri)),
            ldb_publisher,
            log_tasks: Arc::new(Mutex::new(HashMap::new())),
            pods: Arc::new(Mutex::new(HashMap::new())),
            net_state: Arc::new(OnceCell::new()),
            pending_entries: Arc::new(Mutex::new(Vec::new())),
            known_peers: Arc::new(Mutex::new(KnownPeers::new())),
            self_name: Arc::new(self_name),
            tls,
            dns_records,
            vip_aliases: Arc::new(Mutex::new(HashMap::new())),
            service_routes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get a handle for initializing the network state after registration.
    fn network_init_handle(&self) -> NetworkInitHandle {
        NetworkInitHandle {
            net_state: Arc::clone(&self.net_state),
            pending_entries: Arc::clone(&self.pending_entries),
            known_peers: Arc::clone(&self.known_peers),
            self_name: (*self.self_name).clone(),
        }
    }

    /// Handles used by the periodic gossip task.
    fn gossip_handles(&self) -> GossipHandles {
        GossipHandles {
            known_peers: Arc::clone(&self.known_peers),
            self_name: Arc::clone(&self.self_name),
            tls: self.tls.clone(),
        }
    }

    /// Access network state, returning UNAVAILABLE if not yet initialized.
    fn net(&self) -> Result<&NetworkState, scop::tonic::Status> {
        self.net_state.get().ok_or_else(|| {
            scop::tonic::Status::unavailable("network not yet initialized (registration pending)")
        })
    }

    /// Clean up a partially-created pod on failure.
    ///
    /// Stops and removes any containers that were already started, tears down
    /// pod networking, releases the IPAM allocation, and removes the CRI sandbox.
    /// All errors during cleanup are logged but do not propagate.
    async fn cleanup_failed_pod(
        cri: &mut CriClient,
        net: &NetworkState,
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
        let mut ipam = net.ipam.lock().await;
        ipam.release(cri_pod_id);
        let _ = cri.stop_pod_sandbox(cri_pod_id).await;
        let _ = cri.remove_pod_sandbox(cri_pod_id).await;
    }

    /// Convert SCOP PodConfig to CRI PodSandboxConfig.
    fn to_cri_pod_config(
        dns_servers: &[String],
        config: &scop::PodConfig,
    ) -> k8s_cri::v1::PodSandboxConfig {
        cri::pod_sandbox_config(&config.name, &config.environment_qid, dns_servers)
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
        let net = self.net()?;
        let config = request.config.unwrap_or_default();
        let pod_name = config.name.clone();
        let cri_pod_config = Self::to_cri_pod_config(&net.dns_servers, &config);
        let mut cri = self.cri.lock().await;

        // Step 1: Create the CRI pod sandbox (gets its own network namespace)
        let cri_pod_id = cri
            .run_pod_sandbox(cri_pod_config.clone())
            .await
            .map_err(|e| scop::tonic::Status::internal(e.to_string()))?;

        // Step 2: Allocate an IP for this pod
        let ip = {
            let mut ipam = net.ipam.lock().await;
            match ipam.allocate(&cri_pod_id) {
                Ok(ip) => ip,
                Err(e) => {
                    Self::cleanup_failed_pod(&mut cri, net, &cri_pod_id, &[], false).await;
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
                Self::cleanup_failed_pod(&mut cri, net, &cri_pod_id, &[], false).await;
                return Err(scop::tonic::Status::internal(format!(
                    "failed to get network namespace: {e}"
                )));
            }
        };

        // Step 4: Set up pod networking (veth pair, bridge, IP, firewall, egress rules)
        if let Err(e) = net::setup_pod_network(
            &cri_pod_id,
            ip,
            &net.pod_cidr,
            &netns_path,
            net.cluster_cidr.as_deref(),
            net.service_cidr.as_deref(),
        ) {
            Self::cleanup_failed_pod(&mut cri, net, &cri_pod_id, &[], false).await;
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
                Self::cleanup_failed_pod(&mut cri, net, &cri_pod_id, &container_ids, true).await;
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
                    Self::cleanup_failed_pod(&mut cri, net, &cri_pod_id, &container_ids, true)
                        .await;
                    return Err(scop::tonic::Status::internal(format!(
                        "failed to create container {i}: {e}"
                    )));
                }
            };

            // Start the container
            if let Err(e) = cri.start_container(&container_id).await {
                let _ = cri.remove_container(&container_id).await;
                Self::cleanup_failed_pod(&mut cri, net, &cri_pod_id, &container_ids, true).await;
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
            let net = self.net()?;
            let mut ipam = net.ipam.lock().await;
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

    async fn gossip_peers(
        &self,
        request: scop::GossipPeersRequest,
    ) -> Result<scop::GossipPeersResponse, scop::tonic::Status> {
        // If the network isn't ready yet, buffer the entries and return an
        // empty delta. The init handle drains the buffer during `initialize`.
        // Locking order matches NetworkInitHandle::initialize so the TOCTOU
        // between buffer-and-drain is atomic.
        {
            let mut pending = self.pending_entries.lock().await;
            if self.net_state.get().is_none() {
                tracing::info!(
                    from = %request.from_node,
                    count = request.entries.len(),
                    "network not yet initialized, buffering gossip entries"
                );
                pending.extend(request.entries);
                return Ok(scop::GossipPeersResponse { delta: Vec::new() });
            }
        }

        let self_name = self.self_name.as_str();
        let mut known = self.known_peers.lock().await;

        // Merge incoming entries and track which ones produced net-new info.
        let mut changed: Vec<scop::PeerEntry> = Vec::new();
        for entry in request.entries {
            let snapshot = entry.clone();
            if let Err(e) = apply_merge_effect(self_name, &mut known, entry) {
                tracing::warn!(error = %e, "merge failed, dropping entry");
                continue;
            }
            // Re-merge was a no-op indicator only when entry changed the table.
            // `merge_entry` returns None on stale/duplicate entries; we can't
            // distinguish those from metadata-only merges without inspecting
            // the table. For fan-out we prefer correctness over precision: if
            // the entry survived (i.e. is present at the stamp we sent), we
            // forward it. A stale input won't be in the table at its claimed
            // stamp, so it will be filtered out.
            if known.iter().any(|(name, state)| {
                name == &snapshot.node_name
                    && state.last_seen_micros == snapshot.last_seen_micros
                    && state.tombstone == snapshot.tombstone
            }) {
                changed.push(snapshot);
            }
        }

        // Compute delta for the caller from their digest (if any).
        let delta = match request.digest {
            Some(digest) => known.delta_for(&digest, self_name),
            None => Vec::new(),
        };

        drop(known);

        // Schedule reactive fan-out of the changes we accepted. We spawn so
        // the response isn't held up waiting on outbound RPCs.
        if !changed.is_empty() {
            let handles = self.gossip_handles();
            let from_node = request.from_node;
            tokio::spawn(async move {
                reactive_fanout(handles, from_node, changed).await;
            });
        }

        Ok(scop::GossipPeersResponse { delta })
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
        let net = self.net()?;
        let svc_cidr = net.service_cidr.as_deref().unwrap_or("");
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
            tls_ca,
            tls_cert,
            tls_key,
            gossip_fanout,
            gossip_interval_secs,
            tombstone_ttl_secs,
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

            let tls = load_tls(tls_ca, tls_cert, tls_key).await?.map(Arc::new);

            tracing::info!("SCOC conduit starting");
            tracing::info!("  node_name: {}", node_name);
            tracing::info!("  bind: {}", bind);
            tracing::info!("  conduit_address: {}", conduit_address);
            tracing::info!("  orchestrator_address: {}", orchestrator_address);
            tracing::info!("  containerd_socket: {}", containerd_socket);
            tracing::info!("  ldb_brokers: {}", ldb_brokers);
            tracing::info!("  pod_netmask: /{}", pod_netmask);
            tracing::info!("  mtls: {}", tls.is_some());
            tracing::info!("  gossip_fanout: {}", gossip_fanout);
            tracing::info!("  gossip_interval_secs: {}", gossip_interval_secs);
            tracing::info!("  tombstone_ttl_secs: {}", tombstone_ttl_secs);

            GOSSIP_FANOUT.store(gossip_fanout, std::sync::atomic::Ordering::Relaxed);

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

            // Create DNS records table early (needed by CriConduit and DNS server)
            let dns_records = dns::new_records();

            // Create the conduit in pending state (network not yet initialized)
            let conduit = CriConduit::new_pending(
                cri,
                ldb_publisher,
                dns_records.clone(),
                node_name.clone(),
                tls.clone(),
            );
            let init_handle = conduit.network_init_handle();
            let gossip_handles = conduit.gossip_handles();

            // Bind the TCP listener and start the Conduit server BEFORE registering
            // with the orchestrator. This ensures the server is already accepting
            // connections when the plugin sends peer notifications in response to
            // registration.
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tracing::info!("Conduit TCP listener bound on {}", bind);
            let server_tls = tls.clone();
            let server_handle = tokio::spawn(async move {
                scop::serve_conduit_on_tcp_listener(listener, conduit, server_tls.as_deref()).await
            });

            // Connect to orchestrator and register (with retries)
            tracing::info!("Registering with orchestrator at {}", orchestrator_address);
            let mut retries = 0;
            let max_retries = 30;
            let register_response = loop {
                match scop::connect_orchestrator(orchestrator_address.clone(), tls.as_deref()).await
                {
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

            // Inject the orchestrator-supplied seed peers into the buffered
            // gossip entries so `initialize` below merges them in the same
            // drain as any gossip that raced registration. Seeds include live
            // peers and any active tombstones.
            {
                let seed_count = register_response.seed_peers.len();
                tracing::info!(
                    seed_peers = seed_count,
                    "seeding known-peers table from RegisterNodeResponse"
                );
                let mut pending = init_handle.pending_entries.lock().await;
                pending.extend(register_response.seed_peers);
            }

            // Initialize network state and drain the seed peers + any gossip
            // entries that arrived while we were setting up the bridge.
            init_handle
                .initialize(ipam, pod_cidr, cluster_cidr, service_cidr, dns_servers)
                .await?;

            // Spawn the periodic anti-entropy digest task. It exchanges a
            // digest with one random live peer every `gossip_interval_secs`
            // and merges any delta the peer returns.
            {
                let handles = gossip_handles.clone();
                let interval = Duration::from_secs(gossip_interval_secs);
                let tls_for_gossip = tls.clone();
                tokio::spawn(async move {
                    periodic_digest_gossip(handles, interval, tls_for_gossip).await;
                });
            }

            // Spawn the tombstone GC task.
            {
                let known = Arc::clone(&gossip_handles.known_peers);
                let ttl = Duration::from_secs(tombstone_ttl_secs);
                tokio::spawn(async move {
                    tombstone_gc_task(known, ttl).await;
                });
            }

            // Spawn heartbeat task with exponential backoff and re-registration
            let node_name_heartbeat = node_name.clone();
            let orchestrator_address_heartbeat = orchestrator_address.clone();
            let conduit_address_heartbeat = conduit_address.clone();
            let pod_cidr_heartbeat = pod_cidr;
            let tls_heartbeat = tls.clone();
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

                        match scop::connect_orchestrator(
                            orchestrator_address_heartbeat.clone(),
                            tls_heartbeat.as_deref(),
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
                    match scop::connect_orchestrator(
                        orchestrator_address_heartbeat.clone(),
                        tls_heartbeat.as_deref(),
                    )
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
            if let Ok(mut client) =
                scop::connect_orchestrator(orchestrator_address, tls.as_deref()).await
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

/// Resolve the three optional `--tls-*` flags into `Option<TlsMaterial>`.
///
/// Either all three are provided (mTLS enabled) or none (plain gRPC). Any
/// other combination is a startup error.
async fn load_tls(
    ca: Option<PathBuf>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
) -> Result<Option<scop::TlsMaterial>> {
    match (ca, cert, key) {
        (Some(ca), Some(cert), Some(key)) => {
            let material = scop::TlsPaths { ca, cert, key }.load().await?;
            Ok(Some(material))
        }
        (None, None, None) => Ok(None),
        _ => {
            anyhow::bail!(
                "--tls-ca, --tls-cert, and --tls-key must all be provided together, or all omitted"
            )
        }
    }
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

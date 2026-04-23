//! Container Plugin for Skyr
//!
//! This plugin manages container workloads across a cluster of worker nodes.
//! It serves as both:
//! - The Orchestrator, accepting node registrations and connecting to Conduit
//!   services to execute container operations.
//! - An RTP plugin, handling Image, Pod, and other resource lifecycle.
//!
//! Resource types:
//! - `Std/Container.Image` - Container image build via BuildKit
//! - `Std/Container.Pod` - Pod sandbox lifecycle
//! - `Std/Container.Pod.Port` - Pod port (firewall opening / access token)
//! - `Std/Container.Host` - Virtual load balancer with DNS name and VIP
//! - `Std/Container.Host.Port` - Load-balanced port routing (supports pod and host port backends)
//! - `Std/Container.Host.InternetAddress` - Public internet exposure for a Host via floating IP

mod bb3_addr;
mod buildkit;
mod image_name;
mod node_registry;
mod subnet_allocator;
mod vip_allocator;

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use gix_object::tree::EntryKind;
use sclc::{Value, ValueAssertions};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

#[derive(Parser)]
struct Args {
    /// Address to bind the Orchestrator server to.
    #[arg(long, default_value = "0.0.0.0:50053")]
    bind: String,

    /// Canonical, DNS-resolvable hostname the orchestrator uses to identify
    /// itself when initiating overlay-peer gossip. Carried in `GossipPeers`
    /// requests as `from_node` / per-entry `source`, so receivers (and later
    /// hop recipients) can attribute the announcement and exclude us from
    /// reactive fan-out.
    #[arg(long)]
    orchestrator_hostname: String,

    /// How long tombstone records for evicted nodes are retained by the
    /// orchestrator (seconds). SCOCs enforce their own TTL via
    /// `--tombstone-ttl`; this controls how long seed lists continue to
    /// inform newly-joining nodes about recent removals.
    #[arg(long, default_value = "3600")]
    tombstone_ttl_secs: u64,

    /// Maximum number of live peers sent in `RegisterNodeResponse.seed_peers`.
    /// A small sample is enough to bootstrap gossip; anti-entropy fills in
    /// anything missing.
    #[arg(long, default_value = "5")]
    seed_peer_count: usize,

    /// Address to bind the RTP server to.
    #[arg(long, default_value = "tcp://0.0.0.0:50054")]
    rtp_bind: String,

    /// Node registry hostname (Redis).
    #[arg(long)]
    node_registry_hostname: String,

    /// CDB hostname(s) for fetching Git context.
    #[arg(long, value_delimiter = ',')]
    cdb_hostnames: Vec<String>,

    /// BuildKit server address.
    #[arg(long)]
    buildkit_addr: String,

    /// Container registry URL.
    #[arg(long)]
    registry_url: String,

    /// LDB hostname for deployment log streaming.
    #[arg(long)]
    ldb_hostname: String,

    /// Cluster CIDR for pod networking (e.g., "10.42.0.0/16").
    /// Subdivided into per-node subnets during node registration.
    /// Nodes request their preferred subnet size via --pod-netmask.
    #[arg(long, default_value = "10.42.0.0/16")]
    cluster_cidr: String,

    /// Service CIDR for Host VIPs (e.g., "10.43.0.0/16").
    /// Each Host resource gets a VIP from this range.
    #[arg(long, default_value = "10.43.0.0/16")]
    service_cidr: String,

    /// Allow insecure (HTTP) connections to the container registry.
    /// Default is false (HTTPS required).
    #[arg(long, default_value_t = false)]
    insecure_registry: bool,

    /// Path to the PEM-encoded CA certificate used to verify SCOC conduits
    /// and incoming orchestrator clients. When set together with `--tls-cert`
    /// and `--tls-key`, the orchestrator listener and all conduit RPCs use
    /// mTLS. All three flags must be provided together; omit all three to run
    /// plain gRPC.
    #[arg(long)]
    tls_ca: Option<std::path::PathBuf>,
    /// Path to the PEM-encoded leaf certificate for the plugin. The cert
    /// must carry both `serverAuth` and `clientAuth` Extended Key Usages.
    #[arg(long)]
    tls_cert: Option<std::path::PathBuf>,
    /// Path to the PEM-encoded private key matching `--tls-cert`.
    #[arg(long)]
    tls_key: Option<std::path::PathBuf>,
}

// Resource type constants
const IMAGE_RESOURCE_TYPE: &str = "Std/Container.Image";
// Maximum number of concurrent broadcast connections to nodes.
const MAX_BROADCAST_CONCURRENCY: usize = 10;
const POD_RESOURCE_TYPE: &str = "Std/Container.Pod";
const PORT_RESOURCE_TYPE: &str = "Std/Container.Pod.Port";
const ATTACHMENT_RESOURCE_TYPE: &str = "Std/Container.Pod.Attachment";
const HOST_RESOURCE_TYPE: &str = "Std/Container.Host";
const HOST_PORT_RESOURCE_TYPE: &str = "Std/Container.Host.Port";
const HOST_INTERNET_ADDRESS_RESOURCE_TYPE: &str = "Std/Container.Host.InternetAddress";

/// How often the dead-node eviction task runs (seconds).
const EVICTION_SCAN_INTERVAL_SECS: u64 = 60;
/// Nodes with no heartbeat for this long are excluded from scheduling (seconds).
const STALE_HEARTBEAT_THRESHOLD_SECS: u64 = 5 * 60;
/// Nodes with no heartbeat for this long are fully evicted (seconds).
const EVICTION_GRACE_PERIOD_SECS: u64 = 15 * 60;

/// Inner state shared between Orchestrator and RTP servers.
struct ContainerPluginInner {
    /// Node registry client for storing and looking up node addresses.
    node_registry: RwLock<node_registry::Client>,
    /// CDB client for fetching Git context.
    cdb: cdb::Client,
    /// BuildKit server address.
    buildkit_addr: String,
    /// Container registry URL.
    registry_url: String,
    /// Whether to allow insecure (HTTP) registry connections.
    insecure_registry: bool,
    /// LDB publisher for deployment log streaming.
    ldb_publisher: ldb::Publisher,
    /// Allocates per-node subnets from the cluster CIDR.
    subnet_allocator: RwLock<subnet_allocator::SubnetAllocator>,
    /// The cluster-wide CIDR for pod networking, sent to nodes during registration.
    cluster_cidr: String,
    /// The service CIDR for Host VIPs, sent to nodes during registration.
    service_cidr: String,
    /// Allocates VIPs from the service CIDR for Host resources.
    vip_allocator: RwLock<vip_allocator::VipAllocator>,
    /// Nodes that missed a DNS/service-route broadcast and need reconciliation
    /// on next heartbeat. Overlay-peer state is no longer tracked here — it
    /// propagates via the SCOC-to-SCOC gossip protocol.
    nodes_needing_reconciliation: RwLock<HashSet<String>>,
    /// Optional mTLS material used for all conduit RPCs and the orchestrator
    /// listener. `None` means plain gRPC.
    tls: Option<scop::TlsMaterial>,
    /// Canonical hostname used to identify the orchestrator in outbound
    /// `GossipPeers` calls.
    orchestrator_hostname: String,
    /// How long tombstones are retained before being GC'd from Redis.
    tombstone_ttl: Duration,
    /// Maximum number of peers to return in `RegisterNodeResponse.seed_peers`.
    seed_peer_count: usize,
}

/// The container plugin manages connections to worker nodes.
///
/// This is Clone and can be shared across servers via Arc.
#[derive(Clone)]
struct ContainerPlugin {
    inner: Arc<ContainerPluginInner>,
}

impl ContainerPlugin {
    #[allow(clippy::too_many_arguments)]
    fn new(
        node_registry: node_registry::Client,
        cdb: cdb::Client,
        buildkit_addr: String,
        registry_url: String,
        insecure_registry: bool,
        ldb_publisher: ldb::Publisher,
        subnet_allocator: subnet_allocator::SubnetAllocator,
        cluster_cidr: String,
        service_cidr: String,
        vip_allocator: vip_allocator::VipAllocator,
        tls: Option<scop::TlsMaterial>,
        orchestrator_hostname: String,
        tombstone_ttl: Duration,
        seed_peer_count: usize,
    ) -> Self {
        Self {
            inner: Arc::new(ContainerPluginInner {
                node_registry: RwLock::new(node_registry),
                cdb,
                buildkit_addr,
                registry_url,
                insecure_registry,
                ldb_publisher,
                subnet_allocator: RwLock::new(subnet_allocator),
                cluster_cidr,
                service_cidr,
                vip_allocator: RwLock::new(vip_allocator),
                nodes_needing_reconciliation: RwLock::new(HashSet::new()),
                tls,
                orchestrator_hostname,
                tombstone_ttl,
                seed_peer_count,
            }),
        }
    }

    /// Borrow the loaded mTLS material, if any, for use by client RPCs.
    fn tls(&self) -> Option<&scop::TlsMaterial> {
        self.inner.tls.as_ref()
    }

    /// Get a conduit client to a node by name.
    async fn get_conduit(
        &self,
        node_name: &str,
    ) -> Result<scop::ConduitClient<scop::tonic::transport::Channel>, PluginError> {
        // Look up the node address
        let node = {
            let mut registry = self.inner.node_registry.write().await;
            registry
                .get(node_name)
                .await
                .map_err(|e| PluginError::NodeLookup(e.to_string()))?
        };

        // Validate the address looks like a valid endpoint before connecting
        validate_node_address(&node.address)?;

        // Connect to the node's conduit service
        info!(node_name = %node_name, address = %node.address, "connecting to conduit");
        let client = scop::connect_conduit(node.address.clone(), self.tls())
            .await
            .map_err(|e| PluginError::Connect(e.to_string()))?;

        Ok(client)
    }

    /// List all registered nodes.
    async fn list_nodes(&self) -> Result<Vec<node_registry::Node>, PluginError> {
        let mut registry = self.inner.node_registry.write().await;
        registry
            .list()
            .await
            .map_err(|e| PluginError::NodeLookup(e.to_string()))
    }

    /// Select a node for scheduling a pod.
    ///
    /// Picks the first available node that has a recent heartbeat.
    /// Nodes whose last heartbeat exceeds the stale threshold are excluded.
    async fn select_node(&self) -> Result<node_registry::Node, PluginError> {
        let nodes = self.list_nodes().await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let cutoff = now.saturating_sub(STALE_HEARTBEAT_THRESHOLD_SECS);
        nodes
            .into_iter()
            .find(|n| n.last_heartbeat >= cutoff)
            .ok_or(PluginError::NoAvailableNodes)
    }

    // =========================================================================
    // Image Resource Handlers
    // =========================================================================

    /// Create an image by building from a Git context and pushing to the registry.
    ///
    /// Inputs:
    /// - `name`: Image name (without registry prefix)
    /// - `context`: Path to build context directory relative to repo root
    /// - `containerfile`: Path to Containerfile relative to context
    ///
    /// Outputs:
    /// - `fullname`: Full image reference with digest (e.g., "registry:5000/name@sha256:...")
    /// - `digest`: Image digest (e.g., "sha256:...")
    async fn create_image(
        &self,
        deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name: {e}")))?
            .to_string();
        let context_path = inputs
            .get("context")
            .assert_path_ref()
            .map_err(|e| PluginError::InvalidInput(format!("context: {e}")))?;
        let containerfile_path = inputs
            .get("containerfile")
            .assert_path_ref()
            .map_err(|e| PluginError::InvalidInput(format!("containerfile: {e}")))?;

        // Strip leading '/' from paths (they are repo-root-relative)
        let context = context_path.path.trim_start_matches('/').to_string();
        let cf_stripped = containerfile_path.path.trim_start_matches('/');

        // Compute containerfile relative to context for buildkit
        let containerfile = if context.is_empty() {
            cf_stripped.to_string()
        } else {
            cf_stripped
                .strip_prefix(&context)
                .and_then(|s| s.strip_prefix('/'))
                .ok_or_else(|| {
                    PluginError::InvalidInput(
                        "containerfile must be within the context directory".into(),
                    )
                })?
                .to_string()
        };

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            image_name = %name,
            context = %context,
            containerfile = %containerfile,
            deployment_qid = %deployment_qid,
            "creating image"
        );

        let deployment_qid: ids::DeploymentQid = deployment_qid.parse().map_err(|e| {
            PluginError::InvalidInput(format!("invalid deployment QID '{deployment_qid}': {e}"))
        })?;
        let env_qid = deployment_qid.environment_qid();

        // Qualify the image name with <hash>/<org>/<repo>/<name>, each path
        // segment in kebab-case (OCI requires lowercase with [a-z0-9._-]).
        // The leading hash of the original proper name disambiguates inputs
        // whose kebab forms would otherwise collide (e.g. `MyOrg` vs. `my_org`).
        let qualified_name =
            image_name::qualify(env_qid.repo.org.as_str(), env_qid.repo.repo.as_str(), &name);

        // Create a DeploymentClient for this deployment
        let repo_client = self.inner.cdb.repo(deployment_qid.repo_qid().clone());
        let deployment_client = repo_client.deployment(
            deployment_qid.environment_qid().environment.clone(),
            deployment_qid.deployment.clone(),
            deployment_qid.nonce,
        );

        // Extract the Git context to a temporary directory
        let temp_dir = tempfile::tempdir()
            .map_err(|e| PluginError::Internal(format!("failed to create temp dir: {e}")))?;

        // Extract the context directory from the Git tree
        extract_context(&deployment_client, &context, temp_dir.path()).await?;

        debug!(
            temp_dir = %temp_dir.path().display(),
            "extracted git context"
        );

        // Create an LDB namespace publisher for this deployment
        let ldb_namespace = deployment_qid.to_string();
        let log_publisher = self
            .inner
            .ldb_publisher
            .namespace(ldb_namespace)
            .await
            .map_err(|e| PluginError::Internal(format!("failed to create log publisher: {e}")))?;

        // Create an LDB namespace publisher for this resource
        let resource_qid = ids::ResourceQid::new(env_qid.clone(), id.clone());
        let resource_log_publisher = self
            .inner
            .ldb_publisher
            .namespace(resource_qid.to_string())
            .await
            .map_err(|e| {
                PluginError::Internal(format!("failed to create resource log publisher: {e}"))
            })?;

        // Build and push the image
        let result = buildkit::build_and_push(
            &self.inner.buildkit_addr,
            temp_dir.path(),
            &containerfile,
            &qualified_name,
            &self.inner.registry_url,
            self.inner.insecure_registry,
            &log_publisher,
            &resource_log_publisher,
        )
        .await?;

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            fullname = %result.fullname,
            digest = %result.digest,
            "image created"
        );

        // Build outputs
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("fullname"), Value::Str(result.fullname));
        outputs.insert(String::from("digest"), Value::Str(result.digest));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Update an image (rebuild if inputs changed).
    async fn update_image(
        &self,
        deployment_qid: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        // Check if inputs changed
        if inputs_changed(&prev_inputs, &inputs) {
            info!(
                resource_type = %id.typ,
                resource_name = %id.name,
                "image inputs changed, rebuilding"
            );
            return self.create_image(deployment_qid, id, inputs).await;
        }

        // No changes - return existing outputs
        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "image update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Delete an image.
    ///
    /// Note: We don't actually delete the image from the registry, as it may
    /// be referenced by other deployments. The image will be garbage collected
    /// by the registry's own policies.
    async fn delete_image(
        &self,
        id: ids::ResourceId,
        _inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let fullname = outputs.get("fullname").assert_str_ref().unwrap_or("");
        let digest = outputs.get("digest").assert_str_ref().unwrap_or("");

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            fullname = %fullname,
            digest = %digest,
            "image resource deleted (image remains in registry)"
        );

        Ok(())
    }

    // =========================================================================
    // Pod Resource Handlers
    // =========================================================================

    /// Create a pod sandbox on a worker node.
    ///
    /// Inputs:
    /// - `name`: Full resource name including inputs hash (required)
    /// - `containers`: List of `{ image: Str }` records (required)
    ///
    /// Outputs:
    /// - `node`: The node where the pod was scheduled
    /// - `address`: The pod's cluster IP address
    async fn create_pod(
        &self,
        environment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name: {e}")))?
            .to_string();

        // Extract pod-level env vars and containers list
        let pod_envs = extract_env_dict(inputs.get("env"))?;
        let containers = extract_container_configs(inputs.get("containers"), &pod_envs)?;

        // Determine target node
        let node_name = match inputs.get("node") {
            Value::Str(n) => n.clone(),
            Value::Nil => self.select_node().await?.name,
            other => {
                return Err(PluginError::InvalidInput(format!(
                    "node: expected Str? but got {other}"
                ))
                .into());
            }
        };

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            pod_name = %name,
            environment_qid = %environment_qid,
            node = %node_name,
            num_containers = %containers.len(),
            "creating pod"
        );

        // Connect to the target node and create the pod
        let mut conduit = self.get_conduit(&node_name).await?;
        let response = conduit
            .create_pod(scop::CreatePodRequest {
                config: Some(scop::PodConfig {
                    environment_qid: environment_qid.to_string(),
                    name: name.clone(),
                    containers,
                }),
            })
            .await
            .map_err(|e| PluginError::ScopOperation(e.to_string()))?;

        let inner = response.into_inner();
        let address = inner.address;

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            node = %node_name,
            address = %address,
            "pod created"
        );

        // Build outputs
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("node"), Value::Str(node_name));
        outputs.insert(String::from("address"), Value::Str(address));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::from([sclc::Marker::Volatile]),
        })
    }

    /// Update a pod.
    ///
    /// Since the resource name includes the inputs hash, any input change
    /// results in a new resource name. The deployment engine handles the
    /// old→new transition as delete+create. So update is always a no-op.
    async fn update_pod(
        &self,
        _environment_qid: &str,
        id: ids::ResourceId,
        _prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "pod update is a no-op (inputs hash in name ensures delete+create)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
            markers: BTreeSet::from([sclc::Marker::Volatile]),
        })
    }

    /// Delete a pod sandbox.
    async fn delete_pod(
        &self,
        id: ids::ResourceId,
        inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let pod_name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name input: {e}")))?;
        let node_name = outputs
            .get("node")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("node output: {e}")))?;

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            pod_name = %pod_name,
            node = %node_name,
            "deleting pod"
        );

        let mut conduit = self.get_conduit(node_name).await?;

        conduit
            .remove_pod(scop::RemovePodRequest {
                pod_name: pod_name.to_string(),
            })
            .await
            .map_err(|e| PluginError::ScopOperation(format!("remove_pod: {e}")))?;

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            pod_name = %pod_name,
            "pod deleted"
        );

        Ok(())
    }

    // =========================================================================
    // Pod.Port Resource Handlers
    // =========================================================================

    /// Create a port resource on a pod.
    ///
    /// Inputs:
    /// - `podName`: Full resource name with hash (required)
    /// - `ip`: Pod IP address (required)
    /// - `node`: Node where the pod is running (required)
    /// - `port`: Port number (required)
    /// - `protocol`: Protocol, e.g. "tcp" or "udp" (required)
    ///
    /// Outputs: empty `{}`
    async fn create_port(
        &self,
        _environment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let pod_name = inputs
            .get("podName")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podName: {e}")))?
            .to_string();
        let node_name = inputs
            .get("node")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("node: {e}")))?
            .to_string();
        let port = inputs
            .get("port")
            .assert_int_ref()
            .map_err(|e| PluginError::InvalidInput(format!("port: {e}")))?;
        let protocol = inputs
            .get("protocol")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("protocol: {e}")))?
            .to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            pod_name = %pod_name,
            node = %node_name,
            port = %port,
            protocol = %protocol,
            "creating pod port"
        );

        // Open the firewall port on the target node via SCOC
        let mut conduit = self.get_conduit(&node_name).await?;
        conduit
            .open_port(scop::OpenPortRequest {
                pod_name,
                port: *port as i32,
                protocol: protocol.clone(),
            })
            .await
            .map_err(|e| PluginError::Connect(format!("open_port failed: {e}")))?;

        // Build outputs — empty record (Port type fields come from closure/inputs, not plugin)
        let outputs = sclc::Record::default();

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Update a port resource.
    ///
    /// Ports are immutable — any change requires recreating.
    async fn update_port(
        &self,
        environment_qid: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if inputs_changed(&prev_inputs, &inputs) {
            warn!(
                resource_type = %id.typ,
                resource_name = %id.name,
                "pod port inputs changed, recreating"
            );
            self.delete_port(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self.create_port(environment_qid, id, inputs).await;
        }

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "pod port update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Delete a port resource.
    async fn delete_port(
        &self,
        id: ids::ResourceId,
        inputs: sclc::Record,
        _outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let pod_name = inputs
            .get("podName")
            .assert_str_ref()
            .ok()
            .map(String::from);
        let node_name = inputs.get("node").assert_str_ref().ok().map(String::from);
        let port = inputs.get("port").assert_int_ref().ok().copied();
        let protocol = inputs
            .get("protocol")
            .assert_str_ref()
            .ok()
            .map(String::from);

        if let (Some(pod_name), Some(node_name), Some(port), Some(protocol)) =
            (pod_name, node_name, port, protocol)
        {
            match self.get_conduit(&node_name).await {
                Ok(mut conduit) => {
                    if let Err(e) = conduit
                        .close_port(scop::ClosePortRequest {
                            pod_name: pod_name.clone(),
                            port: port as i32,
                            protocol,
                        })
                        .await
                    {
                        warn!(
                            resource_name = %id.name,
                            pod_name = %pod_name,
                            error = %e,
                            "failed to close port (pod may already be gone)"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        resource_name = %id.name,
                        node = %node_name,
                        error = %e,
                        "failed to connect to node for port close (node may be gone)"
                    );
                }
            }
        }

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "pod port deleted"
        );

        Ok(())
    }

    // =========================================================================
    // Attachment Resource Handlers
    // =========================================================================

    /// Create an attachment (open egress port on a pod's firewall).
    ///
    /// Inputs:
    /// - `podName`: Full resource name with hash (required)
    /// - `node`: Node where the pod is running (required)
    /// - `source`: Source pod IP address (required)
    /// - `destination`: Destination IP address (required)
    /// - `port`: Destination port number (required)
    /// - `protocol`: Protocol, "tcp" or "udp" (required)
    async fn create_attachment(
        &self,
        _environment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let pod_name = inputs
            .get("podName")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podName: {e}")))?
            .to_string();
        let node_name = inputs
            .get("node")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("node: {e}")))?
            .to_string();
        let destination = inputs
            .get("destination")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("destination: {e}")))?
            .to_string();
        let port = *inputs
            .get("port")
            .assert_int_ref()
            .map_err(|e| PluginError::InvalidInput(format!("port: {e}")))?;
        let protocol = inputs
            .get("protocol")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("protocol: {e}")))?
            .to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            pod_name = %pod_name,
            node = %node_name,
            destination = %destination,
            port = %port,
            protocol = %protocol,
            "creating attachment"
        );

        let mut conduit = self.get_conduit(&node_name).await?;
        conduit
            .add_attachment(scop::AddAttachmentRequest {
                pod_name,
                destination_address: destination,
                port: port as i32,
                protocol,
            })
            .await
            .map_err(|e| PluginError::ScopOperation(format!("add_attachment: {e}")))?;

        let outputs = sclc::Record::default();

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Update an attachment (immutable — delete old + create new).
    async fn update_attachment(
        &self,
        environment_qid: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if inputs_changed(&prev_inputs, &inputs) {
            self.delete_attachment(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self.create_attachment(environment_qid, id, inputs).await;
        }

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Delete an attachment.
    async fn delete_attachment(
        &self,
        id: ids::ResourceId,
        inputs: sclc::Record,
        _outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let pod_name = inputs
            .get("podName")
            .assert_str_ref()
            .ok()
            .map(String::from);
        let node_name = inputs.get("node").assert_str_ref().ok().map(String::from);
        let destination = inputs
            .get("destination")
            .assert_str_ref()
            .ok()
            .map(String::from);
        let port = inputs.get("port").assert_int_ref().ok().copied();
        let protocol = inputs
            .get("protocol")
            .assert_str_ref()
            .ok()
            .map(String::from);

        if let (Some(pod_name), Some(node_name), Some(destination), Some(port), Some(protocol)) =
            (pod_name, node_name, destination, port, protocol)
        {
            match self.get_conduit(&node_name).await {
                Ok(mut conduit) => {
                    if let Err(e) = conduit
                        .remove_attachment(scop::RemoveAttachmentRequest {
                            pod_name: pod_name.clone(),
                            destination_address: destination,
                            port: port as i32,
                            protocol,
                        })
                        .await
                    {
                        warn!(
                            resource_name = %id.name,
                            pod_name = %pod_name,
                            error = %e,
                            "failed to remove attachment (pod may already be gone)"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        resource_name = %id.name,
                        node = %node_name,
                        error = %e,
                        "failed to connect to node for attachment removal"
                    );
                }
            }
        }

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "attachment deleted"
        );

        Ok(())
    }

    // =========================================================================
    // Host Resource Handlers
    // =========================================================================

    /// Create a Host resource (virtual load balancer with DNS name).
    ///
    /// Inputs:
    /// - `name`: Host name (required). Becomes `{name}.internal` for DNS.
    ///
    /// Outputs:
    /// - `hostname`: The full DNS hostname (e.g., "web-api.internal")
    /// - `vip`: The allocated virtual IP address
    async fn create_host(
        &self,
        _environment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name: {e}")))?
            .to_string();

        let hostname = format!("{name}.internal");

        // Allocate a VIP for this host
        let vip = {
            let mut allocator = self.inner.vip_allocator.write().await;
            allocator
                .allocate(&hostname)
                .map_err(|e| PluginError::Internal(format!("VIP allocation failed: {e}")))?
        };
        let vip_str = vip.to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            hostname = %hostname,
            vip = %vip_str,
            "creating host"
        );

        // Persist VIP allocation in Redis for reconstruction after restart
        {
            let mut registry = self.inner.node_registry.write().await;
            if let Err(e) = registry.store_vip(&hostname, &vip_str).await {
                warn!(hostname = %hostname, error = %e, "failed to persist VIP allocation");
            }
        }

        // Broadcast DNS record to all nodes
        self.broadcast_dns_set(&hostname, &vip_str).await;

        // Build outputs
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("hostname"), Value::Str(hostname));
        outputs.insert(String::from("vip"), Value::Str(vip_str));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Update a Host resource.
    ///
    /// Hosts are immutable — name changes require recreation.
    async fn update_host(
        &self,
        environment_qid: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if inputs_changed(&prev_inputs, &inputs) {
            warn!(
                resource_type = %id.typ,
                resource_name = %id.name,
                "host inputs changed, recreating"
            );
            self.delete_host(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self.create_host(environment_qid, id, inputs).await;
        }

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "host update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Delete a Host resource.
    async fn delete_host(
        &self,
        id: ids::ResourceId,
        _inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let hostname = outputs
            .get("hostname")
            .assert_str_ref()
            .unwrap_or("")
            .to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            hostname = %hostname,
            "deleting host"
        );

        // Broadcast DNS record removal to all nodes
        if !hostname.is_empty() {
            self.broadcast_dns_remove(&hostname).await;

            // Release the VIP
            let mut allocator = self.inner.vip_allocator.write().await;
            allocator.release(&hostname);

            // Remove VIP from Redis
            let mut registry = self.inner.node_registry.write().await;
            if let Err(e) = registry.remove_vip(&hostname).await {
                warn!(hostname = %hostname, error = %e, "failed to remove VIP from registry");
            }
        }

        Ok(())
    }

    // =========================================================================
    // Host.Port Resource Handlers
    // =========================================================================

    /// Create a Host.Port resource (load-balanced port on a Host VIP).
    ///
    /// Inputs:
    /// - `hostHostname`: The Host's DNS hostname (required)
    /// - `hostVip`: The Host's VIP address (required)
    /// - `port`: Port number (required)
    /// - `protocol`: Protocol, "tcp" or "udp" (required)
    /// - `backends`: List of backend port records with address/port/protocol (required)
    ///
    /// Outputs:
    /// - `hostname`: The Host's DNS hostname
    /// - `address`: The Host's VIP address
    /// - `port`: The port number
    /// - `protocol`: The protocol
    async fn create_host_port(
        &self,
        _environment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let host_hostname = inputs
            .get("hostHostname")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("hostHostname: {e}")))?
            .to_string();
        let host_vip = inputs
            .get("hostVip")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("hostVip: {e}")))?
            .to_string();
        let port = *inputs
            .get("port")
            .assert_int_ref()
            .map_err(|e| PluginError::InvalidInput(format!("port: {e}")))?;
        let protocol = inputs
            .get("protocol")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("protocol: {e}")))?
            .to_string();

        // Extract backend port records
        let backends = extract_service_backends(inputs.get("backends"))?;

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            hostname = %host_hostname,
            vip = %host_vip,
            port = %port,
            protocol = %protocol,
            num_backends = %backends.len(),
            "creating host port"
        );

        // Broadcast service route to all nodes
        self.broadcast_service_route_add(&host_vip, port as i32, &protocol, &backends)
            .await;

        // Build outputs
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("hostname"), Value::Str(host_hostname));
        outputs.insert(String::from("address"), Value::Str(host_vip));
        outputs.insert(String::from("port"), Value::Int(port));
        outputs.insert(String::from("protocol"), Value::Str(protocol));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Update a Host.Port resource.
    ///
    /// Host ports are recreated when inputs change (backends, port, etc.).
    async fn update_host_port(
        &self,
        environment_qid: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if inputs_changed(&prev_inputs, &inputs) {
            warn!(
                resource_type = %id.typ,
                resource_name = %id.name,
                "host port inputs changed, recreating"
            );
            self.delete_host_port(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self.create_host_port(environment_qid, id, inputs).await;
        }

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "host port update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Delete a Host.Port resource.
    async fn delete_host_port(
        &self,
        id: ids::ResourceId,
        _inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let host_vip = outputs
            .get("address")
            .assert_str_ref()
            .unwrap_or("")
            .to_string();
        let port = outputs.get("port").assert_int_ref().copied().unwrap_or(0);
        let protocol = outputs
            .get("protocol")
            .assert_str_ref()
            .unwrap_or("tcp")
            .to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            vip = %host_vip,
            port = %port,
            protocol = %protocol,
            "deleting host port"
        );

        // Broadcast service route removal to all nodes
        if !host_vip.is_empty() {
            self.broadcast_service_route_remove(&host_vip, port as i32, &protocol)
                .await;
        }

        Ok(())
    }

    // =========================================================================
    // Host.InternetAddress Resource Handlers
    // =========================================================================

    /// Create a Host.InternetAddress resource (public internet exposure for a Host).
    ///
    /// Inputs:
    /// - `name`: Address name / resource identifier (required)
    /// - `hostHostname`: The parent Host's DNS hostname (required)
    /// - `hostVip`: The parent Host's cluster VIP (required)
    ///
    /// Outputs:
    /// - `publicIp`: The allocated public floating IP
    /// - `lanIp`: The LAN VIP assigned to the SCOC node (internal)
    /// - `node`: The node hosting the VIP (internal)
    async fn create_internet_address(
        &self,
        _environment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name: {e}")))?
            .to_string();
        let host_vip = inputs
            .get("hostVip")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("hostVip: {e}")))?
            .to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            name = %name,
            host_vip = %host_vip,
            "creating internet address"
        );

        // Pick a connected SCOC node to host the LAN VIP
        let node = self.select_node().await?;
        let mut conduit = self.get_conduit(&node.name).await?;

        // Allocate a floating IP + LAN VIP from the BB3 address service
        let alloc = bb3_addr::allocate_address().await?;

        info!(
            resource_name = %id.name,
            floating_ip = %alloc.floating_ip,
            lan_ip = %alloc.lan_ip,
            node = %node.name,
            "assigning VIP to node"
        );

        // Tell the node to add the VIP to its primary interface and DNAT to the Host VIP
        if let Err(e) = conduit
            .add_vip(scop::AddVipRequest {
                address: alloc.lan_ip.clone(),
                destination: host_vip,
            })
            .await
        {
            if let Err(er) = bb3_addr::release_address(&alloc.lan_ip).await {
                info!(
                    resource_name = %id.name,
                    floating_ip = %alloc.floating_ip,
                    lan_ip = %alloc.lan_ip,
                    node = %node.name,
                    "failed to release address after failed add_vip: {er}"
                );
            }

            anyhow::bail!(PluginError::ScopOperation(format!("add_vip: {e}")));
        }

        // Build outputs (lanIp and node are internal, not exposed to SCL)
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("publicIp"), Value::Str(alloc.floating_ip));
        outputs.insert(String::from("lanIp"), Value::Str(alloc.lan_ip));
        outputs.insert(String::from("node"), Value::Str(node.name));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Update a Host.InternetAddress resource.
    ///
    /// Internet addresses are immutable — input changes require recreation.
    async fn update_internet_address(
        &self,
        environment_qid: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if inputs_changed(&prev_inputs, &inputs) {
            warn!(
                resource_type = %id.typ,
                resource_name = %id.name,
                "internet address inputs changed, recreating"
            );
            self.delete_internet_address(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self
                .create_internet_address(environment_qid, id, inputs)
                .await;
        }

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            "internet address update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
            markers: BTreeSet::new(),
        })
    }

    /// Delete a Host.InternetAddress resource.
    async fn delete_internet_address(
        &self,
        id: ids::ResourceId,
        _inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let lan_ip = outputs
            .get("lanIp")
            .assert_str_ref()
            .unwrap_or("")
            .to_string();
        let node_name = outputs
            .get("node")
            .assert_str_ref()
            .unwrap_or("")
            .to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            lan_ip = %lan_ip,
            node = %node_name,
            "deleting internet address"
        );

        // Tell the node to remove the VIP
        if !lan_ip.is_empty() && !node_name.is_empty() {
            match self.get_conduit(&node_name).await {
                Ok(mut conduit) => {
                    if let Err(e) = conduit
                        .remove_vip(scop::RemoveVipRequest {
                            address: lan_ip.clone(),
                        })
                        .await
                    {
                        warn!(
                            lan_ip = %lan_ip,
                            node = %node_name,
                            error = %e,
                            "failed to remove VIP from node"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        node = %node_name,
                        error = %e,
                        "failed to connect to node for VIP removal"
                    );
                }
            }

            // Release the address back to the BB3 service
            if let Err(e) = bb3_addr::release_address(&lan_ip).await {
                warn!(
                    lan_ip = %lan_ip,
                    error = %e,
                    "failed to release address from BB3 service"
                );
            }
        }

        Ok(())
    }

    // =========================================================================
    // Resilience: network reconciliation and dead-node eviction
    // =========================================================================

    /// Reconcile cluster-wide DNS / service-route state after plugin restart.
    ///
    /// Overlay-peer membership is no longer reconciled from here: that state
    /// is owned by the SCOCs and healed by the gossip anti-entropy loop, so
    /// the orchestrator no longer needs to push peer lists at startup.
    async fn reconcile_network_state(&self) {
        let vips = {
            let mut registry = self.inner.node_registry.write().await;
            registry.list_vips().await.unwrap_or_default()
        };

        if vips.is_empty() {
            info!("no DNS records to reconcile after startup");
            return;
        }

        info!(
            dns_records = vips.len(),
            "reconciling DNS records after startup"
        );
        for (host_name, vip_str) in &vips {
            self.broadcast_dns_set(host_name, vip_str).await;
        }
    }

    /// Background task that periodically GC's expired tombstone records from
    /// Redis. `list_tombstones` already filters and deletes expired entries,
    /// so a no-op call on a cadence is enough to bound storage.
    async fn run_tombstone_gc(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(EVICTION_SCAN_INTERVAL_SECS)).await;

            let mut registry = self.inner.node_registry.write().await;
            if let Err(e) = registry.list_tombstones().await {
                warn!(error = %e, "tombstone GC sweep failed");
            }
        }
    }

    /// Background task that periodically scans for dead nodes and evicts them.
    async fn run_dead_node_eviction(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(EVICTION_SCAN_INTERVAL_SECS)).await;

            let stale = {
                let mut registry = self.inner.node_registry.write().await;
                match registry.stale_nodes(EVICTION_GRACE_PERIOD_SECS).await {
                    Ok(nodes) => nodes,
                    Err(e) => {
                        warn!(error = %e, "dead-node eviction scan failed");
                        continue;
                    }
                }
            };

            for node in stale {
                warn!(
                    node = %node.name,
                    last_heartbeat = node.last_heartbeat,
                    "evicting dead node"
                );
                self.evict_node(&node.name).await;
            }
        }
    }

    /// Evict a dead node: release its subnet, unregister from Redis, and
    /// seed a tombstone into the cluster so gossip can spread the removal.
    async fn evict_node(&self, node_name: &str) {
        // Read departing node info before unregistering
        let departing = {
            let mut registry = self.inner.node_registry.write().await;
            registry.get(node_name).await.ok()
        };

        // Release subnet
        {
            let mut allocator = self.inner.subnet_allocator.write().await;
            allocator.release(node_name);
        }

        // Unregister from Redis
        {
            let mut registry = self.inner.node_registry.write().await;
            if let Err(e) = registry.unregister(node_name).await {
                error!(node = %node_name, error = %e, "failed to unregister evicted node");
                return;
            }
        }

        info!(node = %node_name, "evicted dead node");

        // Write and gossip a tombstone so peers drop the node from their
        // overlay tables and so late re-registrations can be reliably ordered
        // against this removal.
        if let Some(node) = departing {
            self.publish_tombstone(&node.name, &node.overlay_endpoint)
                .await;
        }
    }

    /// Mint a tombstone for `node_name` (with its last-known overlay endpoint),
    /// persist it in Redis under the configured TTL, and gossip it to one
    /// random live peer. The tombstone then spreads epidemically via SCOC-to-
    /// SCOC gossip.
    async fn publish_tombstone(&self, node_name: &str, overlay_endpoint: &str) {
        let now = node_registry::now_micros();
        let ttl_micros = (self.inner.tombstone_ttl.as_micros() as u64).max(1);
        let tombstone = node_registry::Tombstone {
            name: node_name.to_string(),
            overlay_endpoint: overlay_endpoint.to_string(),
            last_seen_micros: now,
            expires_at_micros: now.saturating_add(ttl_micros),
        };

        {
            let mut registry = self.inner.node_registry.write().await;
            if let Err(e) = registry.put_tombstone(&tombstone).await {
                warn!(
                    node = %node_name,
                    error = %e,
                    "failed to persist tombstone; peer-removal gossip may be dropped on plugin restart"
                );
            }
        }

        self.seed_gossip(vec![tombstone_to_peer_entry(
            &tombstone,
            &self.inner.orchestrator_hostname,
        )])
        .await;
    }

    /// Push the given gossip entries to a single random live peer, from which
    /// they spread epidemically. This is the orchestrator's only overlay
    /// fan-out: it does not loop over all nodes.
    async fn seed_gossip(&self, entries: Vec<scop::PeerEntry>) {
        if entries.is_empty() {
            return;
        }

        let nodes = match self.list_nodes().await {
            Ok(nodes) => nodes,
            Err(e) => {
                warn!(error = %e, "failed to list nodes for gossip seeding");
                return;
            }
        };

        let Some(target) = pick_gossip_target(&nodes, None) else {
            debug!("no live peers to seed gossip to; entries will propagate on next registration");
            return;
        };

        let address = target.address.clone();
        let target_name = target.name.clone();
        let from_node = self.inner.orchestrator_hostname.clone();
        let tls = self.inner.tls.clone();
        tokio::spawn(async move {
            send_gossip(&address, &target_name, from_node, entries, tls).await;
        });
    }

    /// Push all DNS records to a node.
    /// Used for reconciliation during heartbeats.
    async fn push_dns_records_to_node(&self, node_name: &str) {
        let node = {
            let mut registry = self.inner.node_registry.write().await;
            match registry.get(node_name).await {
                Ok(n) => n,
                Err(e) => {
                    warn!(node = %node_name, error = %e, "failed to get node for DNS reconciliation");
                    return;
                }
            }
        };

        let mut client = match scop::connect_conduit(node.address.clone(), self.tls()).await {
            Ok(c) => c,
            Err(e) => {
                warn!(node = %node_name, error = %e, "failed to connect for DNS reconciliation");
                return;
            }
        };

        let vips = {
            let mut registry = self.inner.node_registry.write().await;
            registry.list_vips().await.unwrap_or_default()
        };

        for (hostname, vip_str) in &vips {
            if let Err(e) = client
                .set_dns_record(scop::SetDnsRecordRequest {
                    hostname: hostname.clone(),
                    address: vip_str.clone(),
                })
                .await
            {
                warn!(
                    node = %node_name,
                    hostname = %hostname,
                    error = %e,
                    "failed to push DNS record"
                );
            }
        }

        debug!(
            node = %node_name,
            dns_records = vips.len(),
            "pushed DNS records to node"
        );
    }

    /// Reconcile a single node by pushing all DNS records to it.
    /// Overlay-peer state is healed by gossip anti-entropy and is no longer
    /// pushed from here.
    #[allow(dead_code)]
    async fn reconcile_single_node(&self, node_name: &str) {
        self.push_dns_records_to_node(node_name).await;

        info!(
            node = %node_name,
            "reconciled node DNS state"
        );
    }

    // =========================================================================
    // Broadcast helpers (send to all registered nodes)
    // =========================================================================

    async fn broadcast_to_nodes<F, Fut>(&self, operation: &str, f: F)
    where
        F: Fn(scop::ConduitClient<scop::tonic::transport::Channel>, String) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<(), scop::tonic::Status>> + Send,
    {
        let nodes = match self.list_nodes().await {
            Ok(nodes) => nodes,
            Err(e) => {
                warn!(error = %e, operation = %operation, "failed to list nodes for broadcast");
                return;
            }
        };

        // Limit concurrent broadcast connections to avoid overwhelming the network
        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_BROADCAST_CONCURRENCY));
        let mut handles = Vec::with_capacity(nodes.len());

        for node in &nodes {
            // Validate the address before connecting
            if let Err(e) = validate_node_address(&node.address) {
                warn!(
                    node = %node.name,
                    operation = %operation,
                    error = %e,
                    "skipping node with invalid address in broadcast"
                );
                // Mark node for reconciliation
                self.inner
                    .nodes_needing_reconciliation
                    .write()
                    .await
                    .insert(node.name.clone());
                continue;
            }

            let semaphore = semaphore.clone();
            let address = node.address.clone();
            let node_name = node.name.clone();
            let operation = operation.to_string();
            let tls = self.inner.tls.clone();

            handles.push(tokio::spawn(async move {
                let _permit = semaphore.acquire().await;
                match scop::connect_conduit(address, tls.as_ref()).await {
                    Ok(client) => (node_name, Ok(client)),
                    Err(e) => {
                        warn!(
                            node = %node_name,
                            operation = %operation,
                            error = %e,
                            "failed to connect to node for broadcast"
                        );
                        (node_name, Err(()))
                    }
                }
            }));
        }

        let mut failed_nodes = Vec::new();
        for handle in handles {
            match handle.await {
                Ok((node_name, Ok(client))) => {
                    if let Err(e) = f(client, node_name.clone()).await {
                        warn!(
                            node = %node_name,
                            operation = %operation,
                            error = %e,
                            "broadcast operation failed on node"
                        );
                        failed_nodes.push(node_name);
                    }
                }
                Ok((node_name, Err(()))) => {
                    failed_nodes.push(node_name);
                }
                Err(_) => {}
            }
        }

        // Track nodes that missed this broadcast for reconciliation on next heartbeat
        if !failed_nodes.is_empty() {
            let mut needs_reconciliation = self.inner.nodes_needing_reconciliation.write().await;
            for name in failed_nodes {
                needs_reconciliation.insert(name);
            }
        }
    }

    async fn broadcast_dns_set(&self, hostname: &str, address: &str) {
        let hostname = hostname.to_string();
        let address = address.to_string();
        self.broadcast_to_nodes("set_dns_record", |mut client, _| {
            let hostname = hostname.clone();
            let address = address.clone();
            async move {
                client
                    .set_dns_record(scop::SetDnsRecordRequest { hostname, address })
                    .await
                    .map(|_| ())
            }
        })
        .await;
    }

    async fn broadcast_dns_remove(&self, hostname: &str) {
        let hostname = hostname.to_string();
        self.broadcast_to_nodes("remove_dns_record", |mut client, _| {
            let hostname = hostname.clone();
            async move {
                client
                    .remove_dns_record(scop::RemoveDnsRecordRequest { hostname })
                    .await
                    .map(|_| ())
            }
        })
        .await;
    }

    async fn broadcast_service_route_add(
        &self,
        vip: &str,
        port: i32,
        protocol: &str,
        backends: &[scop::ServiceBackend],
    ) {
        let vip = vip.to_string();
        let protocol = protocol.to_string();
        let backends = backends.to_vec();
        self.broadcast_to_nodes("add_service_route", |mut client, _| {
            let vip = vip.clone();
            let protocol = protocol.clone();
            let backends = backends.clone();
            async move {
                client
                    .add_service_route(scop::AddServiceRouteRequest {
                        vip,
                        port,
                        protocol,
                        backends,
                    })
                    .await
                    .map(|_| ())
            }
        })
        .await;
    }

    async fn broadcast_service_route_remove(&self, vip: &str, port: i32, protocol: &str) {
        let vip = vip.to_string();
        let protocol = protocol.to_string();
        self.broadcast_to_nodes("remove_service_route", |mut client, _| {
            let vip = vip.clone();
            let protocol = protocol.clone();
            async move {
                client
                    .remove_service_route(scop::RemoveServiceRouteRequest {
                        vip,
                        port,
                        protocol,
                    })
                    .await
                    .map(|_| ())
            }
        })
        .await;
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Validate that a node address looks like a legitimate gRPC endpoint.
///
/// This guards against connecting to arbitrary addresses from compromised Redis data.
/// Expects addresses in the form `http://host:port` or `https://host:port`.
fn validate_node_address(address: &str) -> Result<(), PluginError> {
    // Must start with http:// or https://
    let without_scheme = if let Some(rest) = address.strip_prefix("http://") {
        rest
    } else if let Some(rest) = address.strip_prefix("https://") {
        rest
    } else {
        return Err(PluginError::InvalidInput(format!(
            "node address must start with http:// or https://, got: {address}"
        )));
    };

    // Must have a host:port part with no path traversal or suspicious characters
    let authority = without_scheme.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err(PluginError::InvalidInput(format!(
            "node address has empty authority: {address}"
        )));
    }

    // Must contain a port
    let has_port = if authority.starts_with('[') {
        // IPv6: [::1]:port
        authority.contains("]:")
    } else {
        authority.contains(':')
    };

    if !has_port {
        return Err(PluginError::InvalidInput(format!(
            "node address must include a port: {address}"
        )));
    }

    // Reject addresses containing whitespace or control characters
    if address.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(PluginError::InvalidInput(format!(
            "node address contains invalid characters: {address}"
        )));
    }

    Ok(())
}

/// Extract environment variables from a `Value::Dict` with string keys and values.
///
/// Returns a sorted list of `KeyValue` pairs. Returns an empty vec for `Value::Nil`.
fn extract_env_dict(value: &Value) -> Result<Vec<scop::KeyValue>, PluginError> {
    match value {
        Value::Nil => Ok(vec![]),
        Value::Dict(dict) => {
            let mut envs: Vec<scop::KeyValue> = dict
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::Str(s) => s.clone(),
                        other => {
                            return Err(PluginError::InvalidInput(format!(
                                "env key: expected Str but got {other}"
                            )));
                        }
                    };
                    let value = match v {
                        Value::Str(s) => s.clone(),
                        other => {
                            return Err(PluginError::InvalidInput(format!(
                                "env value for {key}: expected Str but got {other}"
                            )));
                        }
                    };
                    Ok(scop::KeyValue { key, value })
                })
                .collect::<Result<Vec<_>, _>>()?;
            envs.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(envs)
        }
        other => Err(PluginError::InvalidInput(format!(
            "env: expected Dict? but got {other}"
        ))),
    }
}

/// Merge pod-level and container-level env vars into a single list.
///
/// Container-level env vars take precedence over pod-level ones when keys conflict.
fn merge_envs(
    pod_envs: &[scop::KeyValue],
    container_envs: Vec<scop::KeyValue>,
) -> Vec<scop::KeyValue> {
    let mut merged: std::collections::BTreeMap<String, String> = pod_envs
        .iter()
        .map(|kv| (kv.key.clone(), kv.value.clone()))
        .collect();
    for kv in container_envs {
        merged.insert(kv.key, kv.value);
    }
    merged
        .into_iter()
        .map(|(key, value)| scop::KeyValue { key, value })
        .collect()
}

/// Extract container configs from the `containers` input value.
///
/// The containers list is a `Value::List` of records, each with `image` and optional `env` fields.
/// Pod-level env vars are merged with container-level env vars, with container-level taking precedence.
fn extract_container_configs(
    value: &Value,
    pod_envs: &[scop::KeyValue],
) -> Result<Vec<scop::ContainerConfig>, PluginError> {
    match value {
        Value::Nil => Ok(vec![]),
        Value::List(list) => {
            let mut configs = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                let record = item.assert_record_ref().map_err(|e| {
                    PluginError::InvalidInput(format!("containers[{i}]: expected record: {e}"))
                })?;
                let image = record
                    .get("image")
                    .assert_str_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("containers[{i}].image: {e}")))?
                    .to_string();
                let container_envs = extract_env_dict(record.get("env"))
                    .map_err(|e| PluginError::InvalidInput(format!("containers[{i}].env: {e}")))?;
                let envs = merge_envs(pod_envs, container_envs);
                configs.push(scop::ContainerConfig {
                    image,
                    command: vec![],
                    args: vec![],
                    envs,
                });
            }
            Ok(configs)
        }
        other => Err(PluginError::InvalidInput(format!(
            "containers: expected List? but got {other}"
        ))),
    }
}

/// Extract service backends from the `backends` input value.
///
/// The backends list is a `Value::List` of records, each with `address`, `port`,
/// and `protocol` fields (matching Pod.Port or Host.Port output shape).
fn extract_service_backends(value: &Value) -> Result<Vec<scop::ServiceBackend>, PluginError> {
    match value {
        Value::Nil => Ok(vec![]),
        Value::List(list) => {
            let mut backends = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                let record = item.assert_record_ref().map_err(|e| {
                    PluginError::InvalidInput(format!("backends[{i}]: expected record: {e}"))
                })?;
                let address = record
                    .get("address")
                    .assert_str_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("backends[{i}].address: {e}")))?
                    .to_string();
                let port =
                    *record.get("port").assert_int_ref().map_err(|e| {
                        PluginError::InvalidInput(format!("backends[{i}].port: {e}"))
                    })? as i32;
                let protocol = record
                    .get("protocol")
                    .assert_str_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("backends[{i}].protocol: {e}")))?
                    .to_string();
                backends.push(scop::ServiceBackend {
                    address,
                    port,
                    protocol,
                });
            }
            Ok(backends)
        }
        other => Err(PluginError::InvalidInput(format!(
            "backends: expected List? but got {other}"
        ))),
    }
}

/// Extract the host IP from a conduit address like "http://192.168.1.10:50054".
///
/// Validates that the extracted host is a valid IP address to prevent injection
/// of malformed overlay endpoints.
fn extract_overlay_endpoint(addr: &str) -> Option<String> {
    let without_scheme = addr.split("://").nth(1).unwrap_or(addr);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host = if authority.starts_with('[') {
        // IPv6: [::1]:port
        authority
            .split(']')
            .next()
            .unwrap_or(authority)
            .trim_start_matches('[')
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };

    if host.is_empty() {
        return None;
    }

    // Validate that the host is a valid IP address
    if host.parse::<std::net::IpAddr>().is_err() {
        warn!(
            address = %addr,
            host = %host,
            "overlay endpoint is not a valid IP address, using as hostname"
        );
    }

    Some(host.to_owned())
}

/// Check if any inputs have changed between two records.
fn inputs_changed(prev: &sclc::Record, curr: &sclc::Record) -> bool {
    // Simple comparison: serialize and compare
    // This is not the most efficient but is correct
    let prev_json = serde_json::to_string(prev).unwrap_or_default();
    let curr_json = serde_json::to_string(curr).unwrap_or_default();
    prev_json != curr_json
}

fn parse_env_qid_string(deployment_qid: &str) -> Result<String, PluginError> {
    let parsed: ids::DeploymentQid = deployment_qid.parse().map_err(|e| {
        PluginError::InvalidInput(format!("invalid deployment QID '{deployment_qid}': {e}"))
    })?;
    Ok(parsed.environment_qid().to_string())
}

/// Extract a directory from the Git tree to the filesystem.
async fn extract_context(
    client: &cdb::DeploymentClient,
    context_path: &str,
    dest: &Path,
) -> Result<(), PluginError> {
    // Normalize context path (remove leading ./)
    let context_path = context_path.trim_start_matches("./");

    // Get the tree at the context path
    let tree_path = if context_path == "." || context_path.is_empty() {
        None
    } else {
        Some(Path::new(context_path))
    };

    // Read the directory at the context path
    let tree = client
        .read_dir(tree_path)
        .await
        .map_err(|e| PluginError::Internal(format!("failed to read context dir: {e}")))?;

    // Canonicalize the destination to use as the root for symlink validation
    let context_root = dest.canonicalize().map_err(|e| {
        PluginError::Internal(format!(
            "failed to canonicalize dest {}: {e}",
            dest.display()
        ))
    })?;

    // Extract the tree recursively
    extract_tree_recursive(client, &tree, Path::new(context_path), dest, &context_root).await?;

    Ok(())
}

/// Recursively extract a Git tree to the filesystem.
///
/// `context_root` is the canonicalized root directory used to validate that
/// symlink targets do not escape the build context.
async fn extract_tree_recursive(
    client: &cdb::DeploymentClient,
    tree: &gix_object::Tree,
    tree_path: &Path,
    dest: &Path,
    context_root: &Path,
) -> Result<(), PluginError> {
    // Create destination directory
    std::fs::create_dir_all(dest).map_err(|e| {
        PluginError::Internal(format!("failed to create dir {}: {e}", dest.display()))
    })?;

    for entry in tree.entries.iter() {
        let name = std::str::from_utf8(&entry.filename)
            .map_err(|e| PluginError::Internal(format!("invalid utf8 in filename: {e}")))?;
        let entry_dest = dest.join(name);
        let entry_src = tree_path.join(name);

        match entry.mode.kind() {
            EntryKind::Blob | EntryKind::BlobExecutable => {
                // Read file and write to destination
                let data = client.read_file(&entry_src).await.map_err(|e| {
                    PluginError::Internal(format!(
                        "failed to read file {}: {e}",
                        entry_src.display()
                    ))
                })?;

                std::fs::write(&entry_dest, &data).map_err(|e| {
                    PluginError::Internal(format!("failed to write {}: {e}", entry_dest.display()))
                })?;

                // Set executable bit if needed
                #[cfg(unix)]
                if entry.mode.kind() == EntryKind::BlobExecutable {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&entry_dest)
                        .map_err(|e| PluginError::Internal(format!("failed to get metadata: {e}")))?
                        .permissions();
                    perms.set_mode(perms.mode() | 0o111);
                    std::fs::set_permissions(&entry_dest, perms).map_err(|e| {
                        PluginError::Internal(format!("failed to set permissions: {e}"))
                    })?;
                }
            }
            EntryKind::Tree => {
                // Read subtree and recurse
                let subtree = client.read_dir(Some(&entry_src)).await.map_err(|e| {
                    PluginError::Internal(format!(
                        "failed to read dir {}: {e}",
                        entry_src.display()
                    ))
                })?;

                Box::pin(extract_tree_recursive(
                    client,
                    &subtree,
                    &entry_src,
                    &entry_dest,
                    context_root,
                ))
                .await?;
            }
            EntryKind::Link => {
                // Read symlink target (stored as blob content) and create symlink
                let target_data = client.read_file(&entry_src).await.map_err(|e| {
                    PluginError::Internal(format!(
                        "failed to read symlink {}: {e}",
                        entry_src.display()
                    ))
                })?;

                let target = std::str::from_utf8(&target_data)
                    .map_err(|e| PluginError::Internal(format!("invalid utf8 in symlink: {e}")))?;

                // Validate that the symlink target resolves within the context root.
                // Resolve the target relative to the symlink's parent directory.
                let target_path = std::path::Path::new(target);
                let resolved = if target_path.is_absolute() {
                    // Absolute symlinks are never allowed — they escape the context
                    return Err(PluginError::InvalidInput(format!(
                        "symlink {} has absolute target '{}' which escapes the build context",
                        entry_src.display(),
                        target
                    )));
                } else {
                    // Resolve relative to the symlink's parent directory
                    dest.join(target)
                };

                // Normalize by resolving ".." components lexically
                let mut normalized = std::path::PathBuf::new();
                for component in resolved.components() {
                    match component {
                        std::path::Component::ParentDir => {
                            if !normalized.pop() {
                                normalized.push(component);
                            }
                        }
                        _ => normalized.push(component),
                    }
                }

                if !normalized.starts_with(context_root) {
                    return Err(PluginError::InvalidInput(format!(
                        "symlink {} has target '{}' which escapes the build context",
                        entry_src.display(),
                        target
                    )));
                }

                #[cfg(unix)]
                std::os::unix::fs::symlink(target, &entry_dest)
                    .map_err(|e| PluginError::Internal(format!("failed to create symlink: {e}")))?;
            }
            EntryKind::Commit => {
                // Submodule - skip for now
                warn!(
                    path = %entry_dest.display(),
                    "skipping submodule"
                );
            }
        }
    }

    Ok(())
}

/// Resolve the three optional `--tls-*` flags into `Option<TlsMaterial>`.
///
/// Either all three are provided (mTLS enabled) or none (plain gRPC). Any
/// other combination is a startup error.
async fn load_tls(
    ca: Option<std::path::PathBuf>,
    cert: Option<std::path::PathBuf>,
    key: Option<std::path::PathBuf>,
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

/// Send a `GossipPeers` request carrying the given entries to one peer.
///
/// The orchestrator uses this as its sole overlay fan-out: a tombstone or a
/// freshly-registered node's record is handed to one live peer and then
/// propagates epidemically across the cluster via SCOC-to-SCOC gossip.
async fn send_gossip(
    target_address: &str,
    target_name: &str,
    from_node: String,
    entries: Vec<scop::PeerEntry>,
    tls: Option<scop::TlsMaterial>,
) {
    const MAX_RETRIES: u32 = 4;
    const BASE_DELAY: Duration = Duration::from_secs(1);

    for attempt in 0..=MAX_RETRIES {
        let result: Result<(), String> = async {
            let mut client = scop::connect_conduit(target_address.to_string(), tls.as_ref())
                .await
                .map_err(|e| e.to_string())?;
            client
                .gossip_peers(scop::GossipPeersRequest {
                    from_node: from_node.clone(),
                    entries: entries.clone(),
                    digest: None,
                })
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        .await;

        match result {
            Ok(()) => {
                if attempt > 0 {
                    tracing::info!(
                        target = %target_name,
                        attempt = attempt + 1,
                        "gossip seed succeeded after retry"
                    );
                }
                return;
            }
            Err(e) if attempt == MAX_RETRIES => {
                tracing::warn!(
                    target = %target_name,
                    attempt = attempt + 1,
                    error = %e,
                    "gossip seed failed after all retries"
                );
                return;
            }
            Err(e) => {
                let delay = BASE_DELAY * 2u32.pow(attempt);
                tracing::warn!(
                    target = %target_name,
                    attempt = attempt + 1,
                    error = %e,
                    retry_in = ?delay,
                    "gossip seed failed, retrying"
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Pick a single random live peer to seed a gossip message to.
///
/// Skips nodes without a usable overlay endpoint/address and, when provided,
/// the `exclude` hostname (typically the just-registered node itself — we
/// don't want to round-trip its own announcement back to it).
fn pick_gossip_target<'a>(
    nodes: &'a [node_registry::Node],
    exclude: Option<&str>,
) -> Option<&'a node_registry::Node> {
    use rand::seq::SliceRandom;

    let candidates: Vec<&node_registry::Node> = nodes
        .iter()
        .filter(|n| {
            !n.overlay_endpoint.is_empty()
                && !n.address.is_empty()
                && exclude.is_none_or(|name| n.name != name)
        })
        .collect();

    candidates.choose(&mut rand::thread_rng()).copied()
}

/// Build a live-node `PeerEntry` for gossip from a stored `Node`, stamped
/// with `source` = the orchestrator's canonical hostname.
fn node_to_peer_entry(node: &node_registry::Node, source: &str) -> scop::PeerEntry {
    scop::PeerEntry {
        node_name: node.name.clone(),
        overlay_endpoint: node.overlay_endpoint.clone(),
        last_seen_micros: node.overlay_version_micros,
        tombstone: false,
        source: source.to_string(),
    }
}

/// Build a tombstone `PeerEntry` from a stored `Tombstone` record.
fn tombstone_to_peer_entry(tombstone: &node_registry::Tombstone, source: &str) -> scop::PeerEntry {
    scop::PeerEntry {
        node_name: tombstone.name.clone(),
        overlay_endpoint: tombstone.overlay_endpoint.clone(),
        last_seen_micros: tombstone.last_seen_micros,
        tombstone: true,
        source: source.to_string(),
    }
}

#[scop::tonic::async_trait]
impl scop::Orchestrator for ContainerPlugin {
    async fn register_node(
        &self,
        request: scop::RegisterNodeRequest,
    ) -> Result<scop::RegisterNodeResponse, scop::tonic::Status> {
        info!(
            node_name = %request.node_name,
            conduit_address = %request.conduit_address,
            "registering node"
        );

        // Check if this node was previously registered (re-registration after restart).
        // If so, return the same pod_cidr to avoid misalignment with the node's bridge config.
        let existing_node = {
            let mut registry = self.inner.node_registry.write().await;
            registry.get(&request.node_name).await.ok()
        };

        let pod_cidr = if let Some(ref existing) = existing_node
            && !existing.pod_cidr.is_empty()
        {
            // Re-registration: ensure the allocator knows about this existing allocation
            if let Ok(cidr) = existing.pod_cidr.parse::<ipnet::Ipv4Net>() {
                let mut allocator = self.inner.subnet_allocator.write().await;
                // seed is idempotent if already present
                if let Err(e) = allocator.seed(&request.node_name, cidr) {
                    warn!(
                        node_name = %request.node_name,
                        error = %e,
                        "failed to re-seed subnet during re-registration"
                    );
                }
            }
            info!(
                node_name = %request.node_name,
                pod_cidr = %existing.pod_cidr,
                "re-registration: returning previously assigned pod_cidr"
            );
            existing.pod_cidr.clone()
        } else {
            // Fresh registration: allocate a new subnet
            let mut allocator = self.inner.subnet_allocator.write().await;
            match allocator.allocate(&request.node_name, request.pod_netmask as u8) {
                Ok(subnet) => subnet.to_string(),
                Err(e) => {
                    tracing::error!(
                        node_name = %request.node_name,
                        error = %e,
                        "failed to allocate subnet for node"
                    );
                    return Ok(scop::RegisterNodeResponse {
                        success: false,
                        error: e,
                        pod_cidr: String::new(),
                        cluster_cidr: String::new(),
                        service_cidr: String::new(),
                        seed_peers: Vec::new(),
                    });
                }
            }
        };

        let capacity = request.capacity.unwrap_or_default();
        let node_capacity = node_registry::NodeCapacity {
            cpu_millis: capacity.cpu_millis,
            memory_bytes: capacity.memory_bytes,
            max_pods: capacity.max_pods,
        };

        // Extract overlay endpoint (host IP) from conduit address
        let overlay_endpoint =
            extract_overlay_endpoint(&request.conduit_address).unwrap_or_default();

        // Mint a strictly-fresh overlay version stamp so this registration
        // supersedes any tombstone carrying the same name.
        let overlay_version_micros = node_registry::now_micros();

        // List existing nodes and active tombstones before registering. The
        // new node needs both (live peers for seeding; tombstones so it
        // doesn't re-add recently-removed peers it might learn about from
        // stale sources).
        let (existing_nodes, tombstones) = {
            let mut registry = self.inner.node_registry.write().await;
            let nodes = registry.list().await.unwrap_or_default();
            let tombstones = registry.list_tombstones().await.unwrap_or_default();
            (nodes, tombstones)
        };

        let mut registry = self.inner.node_registry.write().await;
        match registry
            .register(
                &request.node_name,
                &request.conduit_address,
                node_capacity,
                request.labels,
                &pod_cidr,
                &overlay_endpoint,
                overlay_version_micros,
            )
            .await
        {
            Ok(node) => {
                // Clear any stale tombstone for this name: the fresh registration
                // supersedes it. SCOCs will drop their local tombstones when the
                // newer live entry arrives via gossip.
                if let Err(e) = registry.delete_tombstone(&request.node_name).await {
                    warn!(
                        node_name = %request.node_name,
                        error = %e,
                        "failed to clear stale tombstone during re-registration"
                    );
                }

                info!(
                    node_name = %node.name,
                    pod_cidr = %pod_cidr,
                    overlay_endpoint = %overlay_endpoint,
                    "node registered successfully"
                );
                // Drop registry lock before gossip
                drop(registry);

                // Build the seed list for the new node: a small random sample
                // of live peers, plus all active tombstones so the new node
                // can't be tricked into re-adding them.
                let seed_peers = build_seed_peers(
                    &existing_nodes,
                    &tombstones,
                    &self.inner.orchestrator_hostname,
                    self.inner.seed_peer_count,
                );

                // Seed one existing peer with the new node's live entry; from
                // there gossip spreads the news epidemically.
                if !overlay_endpoint.is_empty() {
                    let new_entry = scop::PeerEntry {
                        node_name: request.node_name.clone(),
                        overlay_endpoint: overlay_endpoint.clone(),
                        last_seen_micros: overlay_version_micros,
                        tombstone: false,
                        source: self.inner.orchestrator_hostname.clone(),
                    };

                    if let Some(target) =
                        pick_gossip_target(&existing_nodes, Some(&request.node_name))
                    {
                        let address = target.address.clone();
                        let target_name = target.name.clone();
                        let from_node = self.inner.orchestrator_hostname.clone();
                        let tls = self.inner.tls.clone();
                        tokio::spawn(async move {
                            send_gossip(&address, &target_name, from_node, vec![new_entry], tls)
                                .await;
                        });
                    } else {
                        debug!(
                            node_name = %request.node_name,
                            "no live peers to seed with the new node; it is the first member of the cluster"
                        );
                    }
                }

                Ok(scop::RegisterNodeResponse {
                    success: true,
                    error: String::new(),
                    pod_cidr,
                    cluster_cidr: self.inner.cluster_cidr.clone(),
                    service_cidr: self.inner.service_cidr.clone(),
                    seed_peers,
                })
            }
            Err(e) => {
                // Release the subnet on registry failure
                drop(registry);
                let mut allocator = self.inner.subnet_allocator.write().await;
                allocator.release(&request.node_name);

                let error = e.to_string();
                tracing::error!(node_name = %request.node_name, error = %error, "failed to register node");
                Ok(scop::RegisterNodeResponse {
                    success: false,
                    error,
                    pod_cidr: String::new(),
                    cluster_cidr: String::new(),
                    service_cidr: String::new(),
                    seed_peers: Vec::new(),
                })
            }
        }
    }

    async fn heartbeat(
        &self,
        request: scop::HeartbeatRequest,
    ) -> Result<scop::HeartbeatResponse, scop::tonic::Status> {
        let usage = request.usage.map(|u| node_registry::NodeUsage {
            cpu_millis: u.cpu_millis,
            memory_bytes: u.memory_bytes,
            running_pods: u.running_pods,
        });

        let mut registry = self.inner.node_registry.write().await;
        match registry.heartbeat(&request.node_name, usage).await {
            Ok(_) => {
                drop(registry);

                // If this node missed any DNS/service-route broadcast, catch
                // it up now. Overlay-peer state heals via gossip without any
                // orchestrator involvement.
                let needs_dns_reconciliation = {
                    let mut set = self.inner.nodes_needing_reconciliation.write().await;
                    set.remove(&request.node_name)
                };
                if needs_dns_reconciliation {
                    info!(
                        node_name = %request.node_name,
                        "reconciling DNS records for node that missed previous broadcasts"
                    );
                    self.push_dns_records_to_node(&request.node_name).await;
                }

                Ok(scop::HeartbeatResponse { acknowledged: true })
            }
            Err(e) => {
                tracing::warn!(
                    node_name = %request.node_name,
                    error = %e,
                    "heartbeat failed"
                );
                Ok(scop::HeartbeatResponse {
                    acknowledged: false,
                })
            }
        }
    }

    async fn unregister_node(
        &self,
        request: scop::UnregisterNodeRequest,
    ) -> Result<scop::UnregisterNodeResponse, scop::tonic::Status> {
        info!(node_name = %request.node_name, "unregistering node");

        // Read departing node info before unregistering (for overlay endpoint)
        let departing = {
            let mut registry = self.inner.node_registry.write().await;
            registry.get(&request.node_name).await.ok()
        };

        // Release the node's subnet allocation
        {
            let mut allocator = self.inner.subnet_allocator.write().await;
            allocator.release(&request.node_name);
        }

        let mut registry = self.inner.node_registry.write().await;
        match registry.unregister(&request.node_name).await {
            Ok(()) => {
                info!(node_name = %request.node_name, "node unregistered successfully");
                drop(registry);

                // Mint a tombstone and seed one random peer with it; gossip
                // spreads the removal from there.
                if let Some(node) = departing {
                    self.publish_tombstone(&node.name, &node.overlay_endpoint)
                        .await;
                }

                Ok(scop::UnregisterNodeResponse { success: true })
            }
            Err(e) => {
                tracing::error!(
                    node_name = %request.node_name,
                    error = %e,
                    "failed to unregister node"
                );
                Ok(scop::UnregisterNodeResponse { success: false })
            }
        }
    }
}

/// Build the initial seed peer list handed to a freshly-registering node.
///
/// Includes up to `max_live` random live peers and every active tombstone.
/// Tombstones ship with the seed list so the new node doesn't have a window
/// during which it would add a stale re-announcement of an evicted peer.
fn build_seed_peers(
    nodes: &[node_registry::Node],
    tombstones: &[node_registry::Tombstone],
    source: &str,
    max_live: usize,
) -> Vec<scop::PeerEntry> {
    use rand::seq::SliceRandom;

    let mut live_candidates: Vec<&node_registry::Node> = nodes
        .iter()
        .filter(|n| !n.overlay_endpoint.is_empty())
        .collect();
    live_candidates.shuffle(&mut rand::thread_rng());
    live_candidates.truncate(max_live);

    let mut seeds: Vec<scop::PeerEntry> = live_candidates
        .into_iter()
        .map(|n| node_to_peer_entry(n, source))
        .collect();
    seeds.extend(
        tombstones
            .iter()
            .map(|t| tombstone_to_peer_entry(t, source)),
    );
    seeds
}

// =============================================================================
// RTP Plugin Implementation
// =============================================================================

macro_rules! log_on_error {
    ($result:expr, $id:expr, $op:expr) => {{
        if let Err(ref e) = $result {
            error!(
                resource_type = %$id.typ,
                resource_name = %$id.name,
                err = %e,
                "{} failed", $op
            );
        }
        $result
    }};
}

#[async_trait::async_trait]
impl rtp::Plugin for ContainerPlugin {
    async fn create_resource(
        &mut self,
        deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let environment_qid = parse_env_qid_string(deployment_qid)?;
        let environment_qid = environment_qid.as_str();
        let result = match id.typ.as_str() {
            IMAGE_RESOURCE_TYPE => self.create_image(deployment_qid, id.clone(), inputs).await,
            POD_RESOURCE_TYPE => self.create_pod(environment_qid, id.clone(), inputs).await,
            PORT_RESOURCE_TYPE => self.create_port(environment_qid, id.clone(), inputs).await,
            ATTACHMENT_RESOURCE_TYPE => {
                self.create_attachment(environment_qid, id.clone(), inputs)
                    .await
            }
            HOST_RESOURCE_TYPE => self.create_host(environment_qid, id.clone(), inputs).await,
            HOST_PORT_RESOURCE_TYPE => {
                self.create_host_port(environment_qid, id.clone(), inputs)
                    .await
            }
            HOST_INTERNET_ADDRESS_RESOURCE_TYPE => {
                self.create_internet_address(environment_qid, id.clone(), inputs)
                    .await
            }
            _ => return Err(PluginError::UnsupportedResourceType(id.typ.clone()).into()),
        };
        log_on_error!(result, id, "create_resource")
    }

    async fn update_resource(
        &mut self,
        deployment_qid: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let environment_qid = parse_env_qid_string(deployment_qid)?;
        let environment_qid = environment_qid.as_str();
        let result = match id.typ.as_str() {
            IMAGE_RESOURCE_TYPE => {
                self.update_image(
                    deployment_qid,
                    id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
            }
            POD_RESOURCE_TYPE => {
                self.update_pod(
                    environment_qid,
                    id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
            }
            PORT_RESOURCE_TYPE => {
                self.update_port(
                    environment_qid,
                    id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
            }
            ATTACHMENT_RESOURCE_TYPE => {
                self.update_attachment(
                    environment_qid,
                    id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
            }
            HOST_RESOURCE_TYPE => {
                self.update_host(
                    environment_qid,
                    id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
            }
            HOST_PORT_RESOURCE_TYPE => {
                self.update_host_port(
                    environment_qid,
                    id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
            }
            HOST_INTERNET_ADDRESS_RESOURCE_TYPE => {
                self.update_internet_address(
                    environment_qid,
                    id.clone(),
                    prev_inputs,
                    prev_outputs,
                    inputs,
                )
                .await
            }
            _ => return Err(PluginError::UnsupportedResourceType(id.typ.clone()).into()),
        };
        log_on_error!(result, id, "update_resource")
    }

    async fn delete_resource(
        &mut self,
        _deployment_qid: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let result = match id.typ.as_str() {
            IMAGE_RESOURCE_TYPE => self.delete_image(id.clone(), inputs, outputs).await,
            POD_RESOURCE_TYPE => self.delete_pod(id.clone(), inputs, outputs).await,
            PORT_RESOURCE_TYPE => self.delete_port(id.clone(), inputs, outputs).await,
            ATTACHMENT_RESOURCE_TYPE => self.delete_attachment(id.clone(), inputs, outputs).await,
            HOST_RESOURCE_TYPE => self.delete_host(id.clone(), inputs, outputs).await,
            HOST_PORT_RESOURCE_TYPE => self.delete_host_port(id.clone(), inputs, outputs).await,
            HOST_INTERNET_ADDRESS_RESOURCE_TYPE => {
                self.delete_internet_address(id.clone(), inputs, outputs)
                    .await
            }
            _ => return Err(PluginError::UnsupportedResourceType(id.typ.clone()).into()),
        };
        log_on_error!(result, id, "delete_resource")
    }
}

#[derive(Debug, thiserror::Error)]
enum PluginError {
    #[error("node lookup failed: {0}")]
    NodeLookup(String),

    #[error("failed to connect to node: {0}")]
    Connect(String),

    #[error("no available nodes for scheduling")]
    NoAvailableNodes,

    #[error("SCOP operation failed: {0}")]
    ScopOperation(String),

    #[error("unsupported resource type: {0}")]
    UnsupportedResourceType(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("image build failed: {0}")]
    ImageBuild(String),

    #[error("internal error: {0}")]
    Internal(String),
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

    info!("Container plugin starting");
    info!("  orchestrator_bind: {}", args.bind);
    info!("  orchestrator_hostname: {}", args.orchestrator_hostname);
    info!("  rtp_bind: {}", args.rtp_bind);
    info!("  node_registry_hostname: {}", args.node_registry_hostname);
    info!("  cdb_hostnames: {:?}", args.cdb_hostnames);
    info!("  buildkit_addr: {}", args.buildkit_addr);
    info!("  registry_url: {}", args.registry_url);
    info!("  ldb_hostname: {}", args.ldb_hostname);
    info!("  cluster_cidr: {}", args.cluster_cidr);
    info!("  service_cidr: {}", args.service_cidr);
    info!("  insecure_registry: {}", args.insecure_registry);
    info!("  tombstone_ttl_secs: {}", args.tombstone_ttl_secs);
    info!("  seed_peer_count: {}", args.seed_peer_count);

    // Connect to the node registry
    let node_registry = node_registry::ClientBuilder::new()
        .known_node(&args.node_registry_hostname)
        .build()
        .await?;

    info!("Connected to node registry");

    // Connect to CDB
    let mut cdb_builder = cdb::ClientBuilder::new();
    for host in &args.cdb_hostnames {
        cdb_builder = cdb_builder.known_node(host);
    }
    let cdb = cdb_builder.build().await?;

    info!("Connected to CDB");

    // Connect to LDB
    let ldb_publisher = ldb::ClientBuilder::new()
        .brokers(format!("{}:9092", args.ldb_hostname))
        .build_publisher()
        .await?;

    info!("Connected to LDB");

    // Set up subnet allocator for pod networking
    let cluster_cidr: ipnet::Ipv4Net = args
        .cluster_cidr
        .parse()
        .expect("invalid --cluster-cidr, expected CIDR notation (e.g., 10.42.0.0/16)");
    let mut subnet_allocator = subnet_allocator::SubnetAllocator::new(cluster_cidr);

    // Set up VIP allocator for Host resources
    let service_cidr: ipnet::Ipv4Net = args
        .service_cidr
        .parse()
        .expect("invalid --service-cidr, expected CIDR notation (e.g., 10.43.0.0/16)");
    let mut vip_alloc = vip_allocator::VipAllocator::new(service_cidr);

    // Reconstruct allocator state from Redis to prevent address collisions after restart
    {
        let mut registry = node_registry.clone();
        match registry.list().await {
            Ok(nodes) => {
                let count = nodes.len();
                for node in nodes {
                    if !node.pod_cidr.is_empty() {
                        if let Ok(cidr) = node.pod_cidr.parse::<ipnet::Ipv4Net>() {
                            if let Err(e) = subnet_allocator.seed(&node.name, cidr) {
                                warn!(
                                    node = %node.name,
                                    pod_cidr = %node.pod_cidr,
                                    error = %e,
                                    "failed to seed subnet allocation"
                                );
                            }
                        } else {
                            warn!(
                                node = %node.name,
                                pod_cidr = %node.pod_cidr,
                                "invalid pod_cidr in registry, skipping seed"
                            );
                        }
                    }
                }
                info!(
                    seeded_nodes = count,
                    "reconstructed subnet allocator state from registry"
                );
            }
            Err(e) => {
                warn!(error = %e, "failed to list nodes for allocator reconstruction");
            }
        }

        // Reconstruct VIP allocator state from Redis
        match registry.list_vips().await {
            Ok(vips) => {
                let count = vips.len();
                for (host_name, vip_str) in vips {
                    if let Ok(vip) = vip_str.parse::<std::net::Ipv4Addr>() {
                        if let Err(e) = vip_alloc.seed(&host_name, vip) {
                            warn!(
                                host = %host_name,
                                vip = %vip_str,
                                error = %e,
                                "failed to seed VIP allocation"
                            );
                        }
                    } else {
                        warn!(
                            host = %host_name,
                            vip = %vip_str,
                            "invalid VIP in registry, skipping seed"
                        );
                    }
                }
                info!(
                    seeded_vips = count,
                    "reconstructed VIP allocator state from registry"
                );
            }
            Err(e) => {
                warn!(error = %e, "failed to list VIPs for allocator reconstruction");
            }
        }
    }

    // Load optional mTLS material for orchestrator ↔ conduit RPCs
    let tls = load_tls(args.tls_ca, args.tls_cert, args.tls_key).await?;
    info!("  mtls: {}", tls.is_some());

    // Create the plugin (shared between both servers)
    let plugin = ContainerPlugin::new(
        node_registry,
        cdb,
        args.buildkit_addr.clone(),
        args.registry_url.clone(),
        args.insecure_registry,
        ldb_publisher,
        subnet_allocator,
        cluster_cidr.to_string(),
        service_cidr.to_string(),
        vip_alloc,
        tls,
        args.orchestrator_hostname.clone(),
        Duration::from_secs(args.tombstone_ttl_secs),
        args.seed_peer_count,
    );

    // Reconcile network state: rebuild overlay mesh and re-broadcast DNS/routes
    // for nodes that were registered before plugin restart
    plugin.reconcile_network_state().await;

    // Clone for the RTP server (since ContainerPlugin is Clone via Arc)
    let rtp_plugin = plugin.clone();
    let rtp_bind = args.rtp_bind.clone();

    // Spawn dead-node eviction background task
    let eviction_plugin = plugin.clone();
    tokio::spawn(async move {
        eviction_plugin.run_dead_node_eviction().await;
    });

    // Spawn tombstone GC task. Each sweep evicts tombstones whose TTL has
    // elapsed so Redis doesn't grow unbounded over time.
    let tombstone_plugin = plugin.clone();
    tokio::spawn(async move {
        tombstone_plugin.run_tombstone_gc().await;
    });

    // Start the Orchestrator server
    let orchestrator_target = format!("http://{}", args.bind);
    info!("Starting Orchestrator server on {}", args.bind);

    // Start the RTP server
    info!("Starting RTP server on {}", args.rtp_bind);

    // Run both servers concurrently
    let orchestrator_tls = plugin.inner.tls.clone();
    tokio::select! {
        result = scop::serve_orchestrator(&orchestrator_target, plugin, orchestrator_tls.as_ref()) => {
            error!("Orchestrator server exited");
            result?;
        }
        result = rtp::serve(&rtp_bind, move || rtp_plugin.clone()) => {
            error!("RTP server exited");
            result?;
        }
    }

    Ok(())
}

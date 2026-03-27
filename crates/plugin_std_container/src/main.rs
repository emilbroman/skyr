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

mod buildkit;
mod node_registry;
mod subnet_allocator;
mod vip_allocator;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

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
            }),
        }
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
        let client = scop::ConduitClient::connect(node.address.clone())
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
    /// Currently uses a simple strategy: pick the first available node.
    /// Future: implement proper scheduling based on capacity, usage, labels.
    async fn select_node(&self) -> Result<node_registry::Node, PluginError> {
        let nodes = self.list_nodes().await?;
        nodes
            .into_iter()
            .next()
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
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name: {e}")))?
            .to_string();
        let context = inputs
            .get("context")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("context: {e}")))?
            .to_string();
        let containerfile = inputs
            .get("containerfile")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("containerfile: {e}")))?
            .to_string();

        info!(
            resource_type = %id.typ,
            resource_name = %id.name,
            image_name = %name,
            context = %context,
            containerfile = %containerfile,
            environment_qid = %environment_qid,
            deployment_id = %deployment_id,
            "creating image"
        );

        // Parse environment QID to get repository info and construct deployment QID
        let env_qid: ids::EnvironmentQid = environment_qid.parse().map_err(|e| {
            PluginError::InvalidInput(format!("invalid environment QID '{environment_qid}': {e}"))
        })?;
        let deployment: ids::DeploymentId = deployment_id.parse().map_err(|e| {
            PluginError::InvalidInput(format!("invalid deployment ID '{deployment_id}': {e}"))
        })?;
        let deployment_qid = ids::DeploymentQid::new(env_qid.clone(), deployment);

        // Create a DeploymentClient for this deployment
        let repo_client = self.inner.cdb.repo(deployment_qid.repo_qid().clone());
        let deployment_client = repo_client.deployment(
            deployment_qid.environment_qid().environment.clone(),
            deployment_qid.deployment.clone(),
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
        let resource_qid = ids::ResourceQid::new(env_qid, id.clone());
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
            &name,
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
        environment_qid: &str,
        deployment_id: &str,
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
            return self
                .create_image(environment_qid, deployment_id, id, inputs)
                .await;
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
                continue;
            }

            let semaphore = semaphore.clone();
            let address = node.address.clone();
            let node_name = node.name.clone();
            let operation = operation.to_string();

            handles.push(tokio::spawn(async move {
                let _permit = semaphore.acquire().await;
                match scop::ConduitClient::connect(address).await {
                    Ok(client) => (node_name, Some(client)),
                    Err(e) => {
                        warn!(
                            node = %node_name,
                            operation = %operation,
                            error = %e,
                            "failed to connect to node for broadcast"
                        );
                        (node_name, None)
                    }
                }
            }));
        }

        for handle in handles {
            if let Ok((node_name, Some(client))) = handle.await
                && let Err(e) = f(client, node_name.clone()).await
            {
                warn!(
                    node = %node_name,
                    operation = %operation,
                    error = %e,
                    "broadcast operation failed on node"
                );
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

        // Allocate a pod subnet for this node at the requested size
        let pod_cidr = {
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

        // List existing nodes before registering (for peer notification)
        let existing_nodes = {
            let mut registry = self.inner.node_registry.write().await;
            registry.list().await.unwrap_or_default()
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
            )
            .await
        {
            Ok(node) => {
                info!(
                    node_name = %node.name,
                    pod_cidr = %pod_cidr,
                    overlay_endpoint = %overlay_endpoint,
                    "node registered successfully"
                );
                // Drop registry lock before peer notification
                drop(registry);

                // Notify existing nodes about the new peer, and the new node
                // about existing peers (overlay mesh setup)
                if !overlay_endpoint.is_empty() {
                    for existing in &existing_nodes {
                        if existing.overlay_endpoint.is_empty() {
                            continue;
                        }

                        // Tell existing node about new peer
                        match scop::ConduitClient::connect(existing.address.clone()).await {
                            Ok(mut client) => {
                                if let Err(e) = client
                                    .add_overlay_peer(scop::AddOverlayPeerRequest {
                                        peer_host_ip: overlay_endpoint.clone(),
                                    })
                                    .await
                                {
                                    warn!(
                                        node = %existing.name,
                                        peer = %overlay_endpoint,
                                        error = %e,
                                        "failed to notify existing node about new peer"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    node = %existing.name,
                                    error = %e,
                                    "failed to connect to existing node for peer notification"
                                );
                            }
                        }

                        // Tell new node about existing peer
                        match scop::ConduitClient::connect(request.conduit_address.clone()).await {
                            Ok(mut client) => {
                                if let Err(e) = client
                                    .add_overlay_peer(scop::AddOverlayPeerRequest {
                                        peer_host_ip: existing.overlay_endpoint.clone(),
                                    })
                                    .await
                                {
                                    warn!(
                                        node = %request.node_name,
                                        peer = %existing.overlay_endpoint,
                                        error = %e,
                                        "failed to notify new node about existing peer"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    node = %request.node_name,
                                    error = %e,
                                    "failed to connect to new node for peer notification"
                                );
                            }
                        }
                    }
                }

                Ok(scop::RegisterNodeResponse {
                    success: true,
                    error: String::new(),
                    pod_cidr,
                    cluster_cidr: self.inner.cluster_cidr.clone(),
                    service_cidr: self.inner.service_cidr.clone(),
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
            Ok(_) => Ok(scop::HeartbeatResponse { acknowledged: true }),
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
        let departing_endpoint = {
            let mut registry = self.inner.node_registry.write().await;
            registry
                .get(&request.node_name)
                .await
                .ok()
                .map(|n| n.overlay_endpoint.clone())
                .unwrap_or_default()
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

                // Notify remaining nodes to remove the departing peer
                if !departing_endpoint.is_empty() {
                    let remaining_nodes = registry.list().await.unwrap_or_default();
                    drop(registry);

                    for node in &remaining_nodes {
                        if node.overlay_endpoint.is_empty() {
                            continue;
                        }
                        match scop::ConduitClient::connect(node.address.clone()).await {
                            Ok(mut client) => {
                                if let Err(e) = client
                                    .remove_overlay_peer(scop::RemoveOverlayPeerRequest {
                                        peer_host_ip: departing_endpoint.clone(),
                                    })
                                    .await
                                {
                                    warn!(
                                        node = %node.name,
                                        peer = %departing_endpoint,
                                        error = %e,
                                        "failed to notify node about peer removal"
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    node = %node.name,
                                    error = %e,
                                    "failed to connect to node for peer removal"
                                );
                            }
                        }
                    }
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
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let result = match id.typ.as_str() {
            IMAGE_RESOURCE_TYPE => {
                self.create_image(environment_qid, deployment_id, id.clone(), inputs)
                    .await
            }
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
            _ => return Err(PluginError::UnsupportedResourceType(id.typ.clone()).into()),
        };
        log_on_error!(result, id, "create_resource")
    }

    async fn update_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let result = match id.typ.as_str() {
            IMAGE_RESOURCE_TYPE => {
                self.update_image(
                    environment_qid,
                    deployment_id,
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
            _ => return Err(PluginError::UnsupportedResourceType(id.typ.clone()).into()),
        };
        log_on_error!(result, id, "update_resource")
    }

    async fn delete_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: ids::ResourceId,
        inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let _ = (environment_qid, deployment_id);
        let result = match id.typ.as_str() {
            IMAGE_RESOURCE_TYPE => self.delete_image(id.clone(), inputs, outputs).await,
            POD_RESOURCE_TYPE => self.delete_pod(id.clone(), inputs, outputs).await,
            PORT_RESOURCE_TYPE => self.delete_port(id.clone(), inputs, outputs).await,
            ATTACHMENT_RESOURCE_TYPE => self.delete_attachment(id.clone(), inputs, outputs).await,
            HOST_RESOURCE_TYPE => self.delete_host(id.clone(), inputs, outputs).await,
            HOST_PORT_RESOURCE_TYPE => self.delete_host_port(id.clone(), inputs, outputs).await,
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
    info!("  rtp_bind: {}", args.rtp_bind);
    info!("  node_registry_hostname: {}", args.node_registry_hostname);
    info!("  cdb_hostnames: {:?}", args.cdb_hostnames);
    info!("  buildkit_addr: {}", args.buildkit_addr);
    info!("  registry_url: {}", args.registry_url);
    info!("  ldb_hostname: {}", args.ldb_hostname);
    info!("  cluster_cidr: {}", args.cluster_cidr);
    info!("  service_cidr: {}", args.service_cidr);
    info!("  insecure_registry: {}", args.insecure_registry);

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
    let subnet_allocator = subnet_allocator::SubnetAllocator::new(cluster_cidr);

    // Set up VIP allocator for Host resources
    let service_cidr: ipnet::Ipv4Net = args
        .service_cidr
        .parse()
        .expect("invalid --service-cidr, expected CIDR notation (e.g., 10.43.0.0/16)");
    let vip_alloc = vip_allocator::VipAllocator::new(service_cidr);

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
    );

    // Clone for the RTP server (since ContainerPlugin is Clone via Arc)
    let rtp_plugin = plugin.clone();
    let rtp_bind = args.rtp_bind.clone();

    // Start the Orchestrator server
    let orchestrator_target = format!("http://{}", args.bind);
    info!("Starting Orchestrator server on {}", args.bind);

    // Start the RTP server
    info!("Starting RTP server on {}", args.rtp_bind);

    // Run both servers concurrently
    tokio::select! {
        result = scop::serve_orchestrator(&orchestrator_target, plugin) => {
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

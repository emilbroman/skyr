//! Container Plugin for Skyr
//!
//! This plugin manages container workloads across a cluster of worker nodes.
//! It serves as both:
//! - The Orchestrator, accepting node registrations and connecting to Conduit
//!   services to execute container operations.
//! - An RTP plugin, handling Image, Pod, and Container resource lifecycle.
//!
//! Resource types:
//! - `Std/Container.Image` - Container image build via BuildKit
//! - `Std/Container.Pod` - Pod sandbox lifecycle
//! - `Std/Container.Pod.Container` - Container lifecycle within a pod
//! - `Std/Container.Pod.Port` - Pod port (firewall opening / access token)

mod buildkit;
mod node_registry;
mod subnet_allocator;

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
}

// Resource type constants
const IMAGE_RESOURCE_TYPE: &str = "Std/Container.Image";
const POD_RESOURCE_TYPE: &str = "Std/Container.Pod";
const CONTAINER_RESOURCE_TYPE: &str = "Std/Container.Pod.Container";
const PORT_RESOURCE_TYPE: &str = "Std/Container.Pod.Port";

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
    /// LDB publisher for deployment log streaming.
    ldb_publisher: ldb::Publisher,
    /// Allocates per-node subnets from the cluster CIDR.
    subnet_allocator: RwLock<subnet_allocator::SubnetAllocator>,
    /// The cluster-wide CIDR for pod networking, sent to nodes during registration.
    cluster_cidr: String,
}

/// The container plugin manages connections to worker nodes.
///
/// This is Clone and can be shared across servers via Arc.
#[derive(Clone)]
pub struct ContainerPlugin {
    inner: Arc<ContainerPluginInner>,
}

impl ContainerPlugin {
    fn new(
        node_registry: node_registry::Client,
        cdb: cdb::Client,
        buildkit_addr: String,
        registry_url: String,
        ldb_publisher: ldb::Publisher,
        subnet_allocator: subnet_allocator::SubnetAllocator,
        cluster_cidr: String,
    ) -> Self {
        Self {
            inner: Arc::new(ContainerPluginInner {
                node_registry: RwLock::new(node_registry),
                cdb,
                buildkit_addr,
                registry_url,
                ldb_publisher,
                subnet_allocator: RwLock::new(subnet_allocator),
                cluster_cidr,
            }),
        }
    }

    /// Get a conduit client to a node by name.
    pub async fn get_conduit(
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

        // Connect to the node's conduit service
        info!(node_name = %node_name, address = %node.address, "connecting to conduit");
        let client = scop::ConduitClient::connect(node.address.clone())
            .await
            .map_err(|e| PluginError::Connect(e.to_string()))?;

        Ok(client)
    }

    /// List all registered nodes.
    pub async fn list_nodes(&self) -> Result<Vec<node_registry::Node>, PluginError> {
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
            .ok_or_else(|| PluginError::NoAvailableNodes)
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
        id: sclc::ResourceId,
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
            resource_type = %id.ty,
            resource_id = %id.id,
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
        let deployment_qid = ids::DeploymentQid::new(env_qid, deployment);

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

        // Build and push the image
        let result = buildkit::build_and_push(
            &self.inner.buildkit_addr,
            temp_dir.path(),
            &containerfile,
            &name,
            &self.inner.registry_url,
            &log_publisher,
        )
        .await?;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
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
        })
    }

    /// Update an image (rebuild if inputs changed).
    async fn update_image(
        &self,
        environment_qid: &str,
        deployment_id: &str,
        id: sclc::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        // Check if inputs changed
        if inputs_changed(&prev_inputs, &inputs) {
            info!(
                resource_type = %id.ty,
                resource_id = %id.id,
                "image inputs changed, rebuilding"
            );
            return self
                .create_image(environment_qid, deployment_id, id, inputs)
                .await;
        }

        // No changes - return existing outputs
        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            "image update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
        })
    }

    /// Delete an image.
    ///
    /// Note: We don't actually delete the image from the registry, as it may
    /// be referenced by other deployments. The image will be garbage collected
    /// by the registry's own policies.
    async fn delete_image(
        &self,
        id: sclc::ResourceId,
        _inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let fullname = outputs.get("fullname").assert_str_ref().unwrap_or("");
        let digest = outputs.get("digest").assert_str_ref().unwrap_or("");

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
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
    /// - `name`: Pod name (required)
    /// - `uid`: Pod UID (required)
    /// - `node`: Target node name (optional, auto-scheduled if not specified)
    /// - `allow`: List of port resources this pod can reach (optional)
    ///
    /// Outputs:
    /// - `podId`: The CRI pod sandbox ID
    /// - `node`: The node where the pod was scheduled
    /// - `name`: Echo of the pod name
    async fn create_pod(
        &self,
        environment_qid: &str,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name: {e}")))?
            .to_string();
        let uid = inputs
            .get("uid")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("uid: {e}")))?
            .to_string();

        // Extract the allow list (list of port resource records)
        let allowed_destinations = extract_allowed_destinations(inputs.get("allow"))?;

        // Determine target node: use specified node or auto-schedule
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
            resource_type = %id.ty,
            resource_id = %id.id,
            pod_name = %name,
            environment_qid = %environment_qid,
            node = %node_name,
            num_allowed = %allowed_destinations.len(),
            "creating pod sandbox"
        );

        // Connect to the target node and create the pod
        let mut conduit = self.get_conduit(&node_name).await?;
        let response = conduit
            .create_pod(scop::CreatePodRequest {
                config: Some(scop::PodConfig {
                    environment_qid: environment_qid.to_string(),
                    name: name.clone(),
                    uid,
                    allowed_destinations,
                }),
            })
            .await
            .map_err(|e| PluginError::ScopOperation(e.to_string()))?;

        let inner = response.into_inner();
        let pod_id = inner.pod_id;
        let address = inner.address;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            pod_id = %pod_id,
            node = %node_name,
            address = %address,
            "pod sandbox created"
        );

        // Build outputs
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("podId"), Value::Str(pod_id));
        outputs.insert(String::from("node"), Value::Str(node_name));
        outputs.insert(String::from("name"), Value::Str(name));
        outputs.insert(String::from("address"), Value::Str(address));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
        })
    }

    /// Update a pod sandbox.
    ///
    /// Pods are immutable in CRI, so updates that change the pod configuration
    /// would require destroying and recreating. This includes changes to the
    /// name, uid, or allow list (since egress rules are applied at pod creation).
    async fn update_pod(
        &self,
        environment_qid: &str,
        id: sclc::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        // Check if inputs that affect the pod have changed
        // If name, uid, or allow list changed, we need to recreate
        let prev_name = prev_inputs.get("name").assert_str_ref().ok();
        let prev_uid = prev_inputs.get("uid").assert_str_ref().ok();

        let name = inputs.get("name").assert_str_ref().ok();
        let uid = inputs.get("uid").assert_str_ref().ok();

        // Check if the allow list has changed
        let allow_changed = {
            let prev_allow = serde_json::to_string(prev_inputs.get("allow")).unwrap_or_default();
            let curr_allow = serde_json::to_string(inputs.get("allow")).unwrap_or_default();
            prev_allow != curr_allow
        };

        if prev_name != name || prev_uid != uid || allow_changed {
            // Pod identity or network config changed - delete old and create new
            warn!(
                resource_type = %id.ty,
                resource_id = %id.id,
                allow_changed = %allow_changed,
                "pod config changed, recreating"
            );
            self.delete_pod(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self.create_pod(environment_qid, id, inputs).await;
        }

        // No changes that require recreation - return existing outputs
        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            "pod update is a no-op (no recreatable changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
        })
    }

    /// Delete a pod sandbox.
    async fn delete_pod(
        &self,
        id: sclc::ResourceId,
        _inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let pod_id = outputs
            .get("podId")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podId output: {e}")))?;
        let node_name = outputs
            .get("node")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("node output: {e}")))?;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            pod_id = %pod_id,
            node = %node_name,
            "deleting pod sandbox"
        );

        let mut conduit = self.get_conduit(node_name).await?;

        conduit
            .remove_pod(scop::RemovePodRequest {
                pod_id: pod_id.to_string(),
            })
            .await
            .map_err(|e| PluginError::ScopOperation(format!("remove_pod: {e}")))?;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            pod_id = %pod_id,
            "pod sandbox deleted"
        );

        Ok(())
    }

    // =========================================================================
    // Container Resource Handlers
    // =========================================================================

    /// Create a container within a pod.
    ///
    /// Inputs:
    /// - `podId`: The CRI pod sandbox ID (required)
    /// - `podName`: Pod name for metadata (required)
    /// - `podUid`: Pod UID for metadata (required)
    /// - `node`: Node where the pod is running (required)
    /// - `name`: Container name (required)
    /// - `image`: Container image (required)
    /// - `command`: Entrypoint command (optional)
    /// - `args`: Command arguments (optional)
    /// - `envs`: Environment variables as a record (optional)
    ///
    /// Outputs:
    /// - `containerId`: The CRI container ID
    /// - `name`: Echo of the container name
    /// - `image`: Echo of the image
    async fn create_container(
        &self,
        environment_qid: &str,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let pod_id = inputs
            .get("podId")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podId: {e}")))?
            .to_string();
        let pod_name = inputs
            .get("podName")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podName: {e}")))?
            .to_string();
        let pod_uid = inputs
            .get("podUid")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podUid: {e}")))?
            .to_string();
        let node_name = inputs
            .get("node")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("node: {e}")))?
            .to_string();
        let name = inputs
            .get("name")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("name: {e}")))?
            .to_string();
        let image = inputs
            .get("image")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("image: {e}")))?
            .to_string();

        // Extract optional command
        let command = extract_string_list(inputs.get("command"))?;

        // Extract optional args
        let args = extract_string_list(inputs.get("args"))?;

        // Extract optional envs as key-value pairs
        let envs = extract_env_vars(inputs.get("envs"))?;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            container_name = %name,
            image = %image,
            pod_id = %pod_id,
            node = %node_name,
            "creating container"
        );

        let mut conduit = self.get_conduit(&node_name).await?;

        // Create the container
        let create_response = conduit
            .create_container(scop::CreateContainerRequest {
                pod_id: pod_id.clone(),
                config: Some(scop::ContainerConfig {
                    name: name.clone(),
                    image: image.clone(),
                    command,
                    args,
                    envs,
                }),
                pod_config: Some(scop::PodConfig {
                    environment_qid: environment_qid.to_string(),
                    name: pod_name,
                    uid: pod_uid,
                    allowed_destinations: vec![],
                }),
            })
            .await
            .map_err(|e| PluginError::ScopOperation(format!("create_container: {e}")))?;

        let container_id = create_response.into_inner().container_id;

        // Start the container
        conduit
            .start_container(scop::StartContainerRequest {
                container_id: container_id.clone(),
            })
            .await
            .map_err(|e| PluginError::ScopOperation(format!("start_container: {e}")))?;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            container_id = %container_id,
            container_name = %name,
            "container created and started"
        );

        // Build outputs
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("containerId"), Value::Str(container_id));
        outputs.insert(String::from("name"), Value::Str(name));
        outputs.insert(String::from("image"), Value::Str(image));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
        })
    }

    /// Update a container.
    ///
    /// Containers are immutable, so any change requires recreating.
    async fn update_container(
        &self,
        environment_qid: &str,
        id: sclc::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        // Check if any inputs changed
        if inputs_changed(&prev_inputs, &inputs) {
            warn!(
                resource_type = %id.ty,
                resource_id = %id.id,
                "container inputs changed, recreating"
            );
            self.delete_container(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self.create_container(environment_qid, id, inputs).await;
        }

        // No changes - return existing outputs
        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            "container update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
        })
    }

    /// Delete a container.
    async fn delete_container(
        &self,
        id: sclc::ResourceId,
        inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let container_id = outputs
            .get("containerId")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("containerId output: {e}")))?;
        let node_name = inputs
            .get("node")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("node input: {e}")))?;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            container_id = %container_id,
            node = %node_name,
            "deleting container"
        );

        let mut conduit = self.get_conduit(node_name).await?;

        // Stop the container first (with a reasonable timeout)
        conduit
            .stop_container(scop::StopContainerRequest {
                container_id: container_id.to_string(),
                timeout: 30, // 30 seconds timeout
            })
            .await
            .map_err(|e| PluginError::ScopOperation(format!("stop_container: {e}")))?;

        // Then remove it
        conduit
            .remove_container(scop::RemoveContainerRequest {
                container_id: container_id.to_string(),
            })
            .await
            .map_err(|e| PluginError::ScopOperation(format!("remove_container: {e}")))?;

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            container_id = %container_id,
            "container deleted"
        );

        Ok(())
    }

    // =========================================================================
    // Pod.Port Resource Handlers
    // =========================================================================

    /// Create a port resource on a pod.
    ///
    /// Inputs:
    /// - `podName`: Pod name (required)
    /// - `podNamespace`: Pod namespace (required)
    /// - `podAddress`: Pod IP address (required)
    /// - `port`: Port number (required)
    /// - `protocol`: Protocol, e.g. "tcp" or "udp" (required)
    /// - `name`: Optional port name
    ///
    /// Outputs:
    /// - `address`: The pod's IP address
    /// - `port`: The port number
    /// - `protocol`: The protocol
    async fn create_port(
        &self,
        _environment_qid: &str,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        let pod_id = inputs
            .get("podId")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podId: {e}")))?
            .to_string();
        let pod_address = inputs
            .get("podAddress")
            .assert_str_ref()
            .map_err(|e| PluginError::InvalidInput(format!("podAddress: {e}")))?
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
            resource_type = %id.ty,
            resource_id = %id.id,
            pod_id = %pod_id,
            node = %node_name,
            port = %port,
            protocol = %protocol,
            "creating pod port"
        );

        // Open the firewall port on the target node via SCOC
        let mut conduit = self.get_conduit(&node_name).await?;
        conduit
            .open_port(scop::OpenPortRequest {
                pod_id,
                port: *port as i32,
                protocol: protocol.clone(),
            })
            .await
            .map_err(|e| PluginError::Connect(format!("open_port failed: {e}")))?;

        // Build outputs — the port resource records which address/port/protocol
        // is now open.
        let mut outputs = sclc::Record::default();
        outputs.insert(String::from("address"), Value::Str(pod_address));
        outputs.insert(String::from("port"), Value::Int(*port));
        outputs.insert(String::from("protocol"), Value::Str(protocol));

        Ok(sclc::Resource {
            inputs,
            outputs,
            dependencies: vec![],
        })
    }

    /// Update a port resource.
    ///
    /// Ports are immutable — any change requires recreating.
    async fn update_port(
        &self,
        environment_qid: &str,
        id: sclc::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        if inputs_changed(&prev_inputs, &inputs) {
            warn!(
                resource_type = %id.ty,
                resource_id = %id.id,
                "pod port inputs changed, recreating"
            );
            self.delete_port(id.clone(), prev_inputs, prev_outputs)
                .await?;
            return self.create_port(environment_qid, id, inputs).await;
        }

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            "pod port update is a no-op (no changes)"
        );

        Ok(sclc::Resource {
            inputs,
            outputs: prev_outputs,
            dependencies: vec![],
        })
    }

    /// Delete a port resource.
    async fn delete_port(
        &self,
        id: sclc::ResourceId,
        inputs: sclc::Record,
        _outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let pod_id = inputs.get("podId").assert_str_ref().ok().map(String::from);
        let node_name = inputs.get("node").assert_str_ref().ok().map(String::from);
        let port = inputs.get("port").assert_int_ref().ok().copied();
        let protocol = inputs
            .get("protocol")
            .assert_str_ref()
            .ok()
            .map(String::from);

        if let (Some(pod_id), Some(node_name), Some(port), Some(protocol)) =
            (pod_id, node_name, port, protocol)
        {
            match self.get_conduit(&node_name).await {
                Ok(mut conduit) => {
                    if let Err(e) = conduit
                        .close_port(scop::ClosePortRequest {
                            pod_id: pod_id.clone(),
                            port: port as i32,
                            protocol,
                        })
                        .await
                    {
                        warn!(
                            resource_id = %id.id,
                            pod_id = %pod_id,
                            error = %e,
                            "failed to close port (pod may already be gone)"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        resource_id = %id.id,
                        node = %node_name,
                        error = %e,
                        "failed to connect to node for port close (node may be gone)"
                    );
                }
            }
        }

        info!(
            resource_type = %id.ty,
            resource_id = %id.id,
            "pod port deleted"
        );

        Ok(())
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Extract a string list from a Value (for command/args).
fn extract_string_list(value: &Value) -> Result<Vec<String>, PluginError> {
    match value {
        Value::Nil => Ok(vec![]),
        Value::List(list) => {
            let mut result = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                let s = item
                    .assert_str_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("list item {i}: {e}")))?;
                result.push(s.to_string());
            }
            Ok(result)
        }
        other => Err(PluginError::InvalidInput(format!(
            "expected List? but got {other}"
        ))),
    }
}

/// Extract environment variables from a Value (record of string values).
fn extract_env_vars(value: &Value) -> Result<Vec<scop::KeyValue>, PluginError> {
    match value {
        Value::Nil => Ok(vec![]),
        Value::Record(record) => {
            let mut envs = Vec::with_capacity(record.iter().count());
            for (key, val) in record.iter() {
                let v = val
                    .assert_str_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("env var {key}: {e}")))?;
                envs.push(scop::KeyValue {
                    key: key.to_string(),
                    value: v.to_string(),
                });
            }
            Ok(envs)
        }
        other => Err(PluginError::InvalidInput(format!(
            "expected Record? for envs but got {other}"
        ))),
    }
}

/// Extract allowed destinations from the `allow` input value.
///
/// The allow list is a `Value::List` of records, each with `address`, `port`,
/// and `protocol` fields (matching Pod.Port output shape). Returns an empty
/// list if the value is Nil (allow not provided).
fn extract_allowed_destinations(
    value: &Value,
) -> Result<Vec<scop::AllowedDestination>, PluginError> {
    match value {
        Value::Nil => Ok(vec![]),
        Value::List(list) => {
            let mut destinations = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                let record = item.assert_record_ref().map_err(|e| {
                    PluginError::InvalidInput(format!("allow[{i}]: expected record: {e}"))
                })?;
                let address = record
                    .get("address")
                    .assert_str_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("allow[{i}].address: {e}")))?
                    .to_string();
                let port = *record
                    .get("port")
                    .assert_int_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("allow[{i}].port: {e}")))?
                    as i32;
                let protocol = record
                    .get("protocol")
                    .assert_str_ref()
                    .map_err(|e| PluginError::InvalidInput(format!("allow[{i}].protocol: {e}")))?
                    .to_string();
                destinations.push(scop::AllowedDestination {
                    address,
                    port,
                    protocol,
                });
            }
            Ok(destinations)
        }
        other => Err(PluginError::InvalidInput(format!(
            "allow: expected List? but got {other}"
        ))),
    }
}

/// Extract the host IP from a conduit address like "http://192.168.1.10:50054".
fn extract_overlay_endpoint(addr: &str) -> Option<String> {
    let without_scheme = addr.split("://").nth(1).unwrap_or(addr);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    if authority.starts_with('[') {
        // IPv6
        Some(
            authority
                .split(']')
                .next()
                .unwrap_or(authority)
                .trim_start_matches('[')
                .to_owned(),
        )
    } else {
        let host = authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority);
        if host.is_empty() {
            None
        } else {
            Some(host.to_owned())
        }
    }
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
        Some(std::path::Path::new(context_path))
    };

    // Read the directory at the context path
    let tree = client
        .read_dir(tree_path)
        .await
        .map_err(|e| PluginError::Internal(format!("failed to read context dir: {e}")))?;

    // Extract the tree recursively
    let context_path_buf = std::path::PathBuf::from(context_path);
    extract_tree_recursive(client, &tree, &context_path_buf, dest).await?;

    Ok(())
}

/// Recursively extract a Git tree to the filesystem.
async fn extract_tree_recursive(
    client: &cdb::DeploymentClient,
    tree: &gix_object::Tree,
    tree_path: &std::path::Path,
    dest: &std::path::Path,
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

#[async_trait::async_trait]
impl rtp::Plugin for ContainerPlugin {
    async fn create_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: sclc::ResourceId,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.ty.as_str() {
            IMAGE_RESOURCE_TYPE => {
                let result = self
                    .create_image(environment_qid, deployment_id, id.clone(), inputs)
                    .await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "image create_resource failed"
                    );
                }
                result
            }
            POD_RESOURCE_TYPE => {
                let result = self.create_pod(environment_qid, id.clone(), inputs).await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "pod create_resource failed"
                    );
                }
                result
            }
            CONTAINER_RESOURCE_TYPE => {
                let result = self
                    .create_container(environment_qid, id.clone(), inputs)
                    .await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "container create_resource failed"
                    );
                }
                result
            }
            PORT_RESOURCE_TYPE => {
                let result = self.create_port(environment_qid, id.clone(), inputs).await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "port create_resource failed"
                    );
                }
                result
            }
            _ => Err(PluginError::UnsupportedResourceType(id.ty.clone()).into()),
        }
    }

    async fn update_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: sclc::ResourceId,
        prev_inputs: sclc::Record,
        prev_outputs: sclc::Record,
        inputs: sclc::Record,
    ) -> anyhow::Result<sclc::Resource> {
        match id.ty.as_str() {
            IMAGE_RESOURCE_TYPE => {
                let result = self
                    .update_image(
                        environment_qid,
                        deployment_id,
                        id.clone(),
                        prev_inputs,
                        prev_outputs,
                        inputs,
                    )
                    .await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "image update_resource failed"
                    );
                }
                result
            }
            POD_RESOURCE_TYPE => {
                let result = self
                    .update_pod(
                        environment_qid,
                        id.clone(),
                        prev_inputs,
                        prev_outputs,
                        inputs,
                    )
                    .await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "pod update_resource failed"
                    );
                }
                result
            }
            CONTAINER_RESOURCE_TYPE => {
                let result = self
                    .update_container(
                        environment_qid,
                        id.clone(),
                        prev_inputs,
                        prev_outputs,
                        inputs,
                    )
                    .await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "container update_resource failed"
                    );
                }
                result
            }
            PORT_RESOURCE_TYPE => {
                let result = self
                    .update_port(
                        environment_qid,
                        id.clone(),
                        prev_inputs,
                        prev_outputs,
                        inputs,
                    )
                    .await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "port update_resource failed"
                    );
                }
                result
            }
            _ => Err(PluginError::UnsupportedResourceType(id.ty.clone()).into()),
        }
    }

    async fn delete_resource(
        &mut self,
        environment_qid: &str,
        deployment_id: &str,
        id: sclc::ResourceId,
        inputs: sclc::Record,
        outputs: sclc::Record,
    ) -> anyhow::Result<()> {
        let _ = (environment_qid, deployment_id);
        match id.ty.as_str() {
            IMAGE_RESOURCE_TYPE => {
                let result = self.delete_image(id.clone(), inputs, outputs).await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "image delete_resource failed"
                    );
                }
                result
            }
            POD_RESOURCE_TYPE => {
                let result = self.delete_pod(id.clone(), inputs, outputs).await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "pod delete_resource failed"
                    );
                }
                result
            }
            CONTAINER_RESOURCE_TYPE => {
                let result = self.delete_container(id.clone(), inputs, outputs).await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "container delete_resource failed"
                    );
                }
                result
            }
            PORT_RESOURCE_TYPE => {
                let result = self.delete_port(id.clone(), inputs, outputs).await;
                if let Err(ref e) = result {
                    error!(
                        resource_type = %id.ty,
                        resource_id = %id.id,
                        err = %e,
                        "port delete_resource failed"
                    );
                }
                result
            }
            _ => Err(PluginError::UnsupportedResourceType(id.ty.clone()).into()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
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

    // Create the plugin (shared between both servers)
    let plugin = ContainerPlugin::new(
        node_registry,
        cdb,
        args.buildkit_addr.clone(),
        args.registry_url.clone(),
        ldb_publisher,
        subnet_allocator,
        cluster_cidr.to_string(),
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

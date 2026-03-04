//! CRI (Container Runtime Interface) client implementation.
//!
//! This module provides a client for communicating with containerd via the CRI gRPC API.

use std::path::Path;

use anyhow::{Context, Result};
use hyper_util::rt::TokioIo;
use k8s_cri::v1::{
    ContainerConfig, ContainerMetadata, CreateContainerRequest, CreateContainerResponse,
    LinuxContainerConfig, LinuxContainerSecurityContext, LinuxPodSandboxConfig,
    LinuxSandboxSecurityContext, NamespaceMode, NamespaceOption, PodSandboxConfig,
    PodSandboxMetadata, RemoveContainerRequest, RemovePodSandboxRequest, RunPodSandboxRequest,
    RunPodSandboxResponse, StartContainerRequest, StopContainerRequest, StopPodSandboxRequest,
    VersionRequest, image_service_client::ImageServiceClient,
    runtime_service_client::RuntimeServiceClient,
};
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use tracing::{debug, info};

/// A CRI client connected to a container runtime via Unix socket.
#[derive(Clone)]
pub struct CriClient {
    runtime: RuntimeServiceClient<Channel>,
    images: ImageServiceClient<Channel>,
}

impl CriClient {
    /// Connect to the containerd CRI socket.
    pub async fn connect(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_owned();
        info!(socket = %socket_path.display(), "connecting to CRI socket");

        // Create a channel that connects via Unix socket
        let channel = Endpoint::try_from("http://[::]:0")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = socket_path.clone();
                async move { UnixStream::connect(path).await.map(TokioIo::new) }
            }))
            .await
            .context("failed to connect to CRI socket")?;

        let runtime = RuntimeServiceClient::new(channel.clone());
        let images = ImageServiceClient::new(channel);

        Ok(Self { runtime, images })
    }

    /// Check CRI version and connectivity.
    pub async fn version(&mut self) -> Result<String> {
        let response = self
            .runtime
            .version(VersionRequest {
                version: "v1".to_string(),
            })
            .await
            .context("version request failed")?
            .into_inner();

        info!(
            runtime_name = %response.runtime_name,
            runtime_version = %response.runtime_version,
            "connected to container runtime"
        );

        Ok(response.runtime_version)
    }

    /// Create and start a pod sandbox.
    pub async fn run_pod_sandbox(&mut self, config: PodSandboxConfig) -> Result<String> {
        let pod_name = config
            .metadata
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        debug!(pod = %pod_name, "creating pod sandbox");

        let response: RunPodSandboxResponse = self
            .runtime
            .run_pod_sandbox(RunPodSandboxRequest {
                config: Some(config),
                runtime_handler: String::new(),
            })
            .await
            .context("run_pod_sandbox failed")?
            .into_inner();

        info!(pod = %pod_name, sandbox_id = %response.pod_sandbox_id, "pod sandbox created");
        Ok(response.pod_sandbox_id)
    }

    /// Stop a pod sandbox.
    pub async fn stop_pod_sandbox(&mut self, sandbox_id: &str) -> Result<()> {
        debug!(sandbox_id = %sandbox_id, "stopping pod sandbox");

        self.runtime
            .stop_pod_sandbox(StopPodSandboxRequest {
                pod_sandbox_id: sandbox_id.to_string(),
            })
            .await
            .context("stop_pod_sandbox failed")?;

        info!(sandbox_id = %sandbox_id, "pod sandbox stopped");
        Ok(())
    }

    /// Remove a pod sandbox.
    pub async fn remove_pod_sandbox(&mut self, sandbox_id: &str) -> Result<()> {
        debug!(sandbox_id = %sandbox_id, "removing pod sandbox");

        self.runtime
            .remove_pod_sandbox(RemovePodSandboxRequest {
                pod_sandbox_id: sandbox_id.to_string(),
            })
            .await
            .context("remove_pod_sandbox failed")?;

        info!(sandbox_id = %sandbox_id, "pod sandbox removed");
        Ok(())
    }

    /// Create a container within a pod sandbox.
    pub async fn create_container(
        &mut self,
        sandbox_id: &str,
        sandbox_config: &PodSandboxConfig,
        container_config: ContainerConfig,
    ) -> Result<String> {
        let container_name = container_config
            .metadata
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "unknown".to_string());
        debug!(
            sandbox_id = %sandbox_id,
            container = %container_name,
            "creating container"
        );

        let response: CreateContainerResponse = self
            .runtime
            .create_container(CreateContainerRequest {
                pod_sandbox_id: sandbox_id.to_string(),
                config: Some(container_config),
                sandbox_config: Some(sandbox_config.clone()),
            })
            .await
            .context("create_container failed")?
            .into_inner();

        info!(
            sandbox_id = %sandbox_id,
            container = %container_name,
            container_id = %response.container_id,
            "container created"
        );
        Ok(response.container_id)
    }

    /// Start a container.
    pub async fn start_container(&mut self, container_id: &str) -> Result<()> {
        debug!(container_id = %container_id, "starting container");

        self.runtime
            .start_container(StartContainerRequest {
                container_id: container_id.to_string(),
            })
            .await
            .context("start_container failed")?;

        info!(container_id = %container_id, "container started");
        Ok(())
    }

    /// Stop a container.
    pub async fn stop_container(&mut self, container_id: &str, timeout: i64) -> Result<()> {
        debug!(container_id = %container_id, timeout = timeout, "stopping container");

        self.runtime
            .stop_container(StopContainerRequest {
                container_id: container_id.to_string(),
                timeout,
            })
            .await
            .context("stop_container failed")?;

        info!(container_id = %container_id, "container stopped");
        Ok(())
    }

    /// Remove a container.
    pub async fn remove_container(&mut self, container_id: &str) -> Result<()> {
        debug!(container_id = %container_id, "removing container");

        self.runtime
            .remove_container(RemoveContainerRequest {
                container_id: container_id.to_string(),
            })
            .await
            .context("remove_container failed")?;

        info!(container_id = %container_id, "container removed");
        Ok(())
    }

    /// Get a reference to the image service client for image operations.
    pub fn images(&mut self) -> &mut ImageServiceClient<Channel> {
        &mut self.images
    }

    /// Get a reference to the runtime service client for advanced operations.
    pub fn runtime(&mut self) -> &mut RuntimeServiceClient<Channel> {
        &mut self.runtime
    }
}

/// Helper to create a minimal pod sandbox config for testing.
/// Uses host namespaces to work in nested container environments.
pub fn test_pod_config(name: &str, namespace: &str) -> PodSandboxConfig {
    PodSandboxConfig {
        metadata: Some(PodSandboxMetadata {
            name: name.to_string(),
            uid: format!("{name}-{namespace}-uid"),
            namespace: namespace.to_string(),
            attempt: 0,
        }),
        // Empty hostname to avoid UTS namespace requirement
        hostname: String::new(),
        log_directory: format!("/var/log/pods/{namespace}_{name}"),
        dns_config: None,
        port_mappings: vec![],
        labels: Default::default(),
        annotations: Default::default(),
        linux: Some(LinuxPodSandboxConfig {
            cgroup_parent: String::new(),
            #[allow(deprecated)]
            security_context: Some(LinuxSandboxSecurityContext {
                namespace_options: Some(NamespaceOption {
                    // Use host namespaces for nested container compatibility
                    network: NamespaceMode::Node.into(),
                    pid: NamespaceMode::Node.into(),
                    ipc: NamespaceMode::Node.into(),
                    target_id: String::new(),
                    // Don't use user namespaces
                    userns_options: None,
                }),
                selinux_options: None,
                run_as_user: None,
                run_as_group: None,
                readonly_rootfs: false,
                supplemental_groups: vec![],
                privileged: true,
                seccomp: None,
                apparmor: None,
                supplemental_groups_policy: 0,
                // Deprecated field, but still required by the struct
                seccomp_profile_path: String::new(),
            }),
            sysctls: Default::default(),
            overhead: None,
            resources: None,
        }),
        windows: None,
    }
}

/// Helper to create a minimal container config for testing.
pub fn test_container_config(name: &str, image: &str) -> ContainerConfig {
    ContainerConfig {
        metadata: Some(ContainerMetadata {
            name: name.to_string(),
            attempt: 0,
        }),
        image: Some(k8s_cri::v1::ImageSpec {
            image: image.to_string(),
            annotations: Default::default(),
            user_specified_image: String::new(),
            runtime_handler: String::new(),
        }),
        command: vec![],
        args: vec![],
        working_dir: String::new(),
        envs: vec![],
        mounts: vec![],
        devices: vec![],
        labels: Default::default(),
        annotations: Default::default(),
        log_path: String::new(),
        stdin: false,
        stdin_once: false,
        tty: false,
        linux: Some(LinuxContainerConfig {
            resources: None,
            #[allow(deprecated)]
            security_context: Some(LinuxContainerSecurityContext {
                capabilities: None,
                privileged: true,
                namespace_options: Some(NamespaceOption {
                    // Use host namespaces for nested container compatibility
                    network: NamespaceMode::Node.into(),
                    pid: NamespaceMode::Node.into(),
                    ipc: NamespaceMode::Node.into(),
                    target_id: String::new(),
                    // Don't use user namespaces
                    userns_options: None,
                }),
                selinux_options: None,
                run_as_user: None,
                run_as_username: String::new(),
                run_as_group: None,
                readonly_rootfs: false,
                supplemental_groups: vec![],
                no_new_privs: false,
                masked_paths: vec![],
                readonly_paths: vec![],
                seccomp: None,
                apparmor: None,
                supplemental_groups_policy: 0,
                // Deprecated fields, but still required by the struct
                apparmor_profile: String::new(),
                seccomp_profile_path: String::new(),
            }),
        }),
        windows: None,
        cdi_devices: vec![],
    }
}

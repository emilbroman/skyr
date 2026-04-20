use anyhow::anyhow;
use russh::{Channel, server};
use tokio::sync::mpsc;

use crate::auth::ensure_repo_access;
use crate::{ChannelMessage, UserFacingError};

/// Port-forward resource types that we support.
const POD_PORT_TYPE: &str = "Std/Container.Pod.Port";
const HOST_PORT_TYPE: &str = "Std/Container.Host.Port";
const POD_TYPE: &str = "Std/Container.Pod";

/// Extract port-forward target from a Pod.Port resource's inputs.
fn resolve_pod_port_target(inputs: &sclc::Record) -> anyhow::Result<(String, String, i32, String)> {
    let pod_name = match inputs.get("podName") {
        sclc::Value::Str(s) => s.clone(),
        _ => return Err(anyhow!("resource inputs missing 'podName'")),
    };
    let node_name = match inputs.get("node") {
        sclc::Value::Str(s) => s.clone(),
        _ => return Err(anyhow!("resource inputs missing 'node'")),
    };
    let port: i32 = match inputs.get("port") {
        sclc::Value::Int(n) => *n as i32,
        _ => return Err(anyhow!("resource inputs missing 'port'")),
    };
    let protocol = match inputs.get("protocol") {
        sclc::Value::Str(s) => s.clone(),
        _ => "tcp".to_string(),
    };
    Ok((pod_name, node_name, port, protocol))
}

/// Resolve a Host.Port resource to a concrete pod target by picking the first
/// backend and looking up the corresponding Pod resource to find the node.
async fn resolve_host_port_target(
    inputs: &sclc::Record,
    ns: &rdb::NamespaceClient,
) -> anyhow::Result<(String, String, i32, String)> {
    let backends = match inputs.get("backends") {
        sclc::Value::List(list) => list,
        _ => return Err(anyhow!("Host.Port resource inputs missing 'backends'")),
    };

    let backend = backends
        .first()
        .ok_or_else(|| anyhow!("Host.Port has no backends"))?;

    let backend = match backend {
        sclc::Value::Record(r) => r,
        _ => return Err(anyhow!("Host.Port backend is not a record")),
    };

    let address = match backend.get("address") {
        sclc::Value::Str(s) => s.clone(),
        _ => return Err(anyhow!("Host.Port backend missing 'address'")),
    };
    let port: i32 = match backend.get("port") {
        sclc::Value::Int(n) => *n as i32,
        _ => return Err(anyhow!("Host.Port backend missing 'port'")),
    };
    let protocol = match backend.get("protocol") {
        sclc::Value::Str(s) => s.clone(),
        _ => "tcp".to_string(),
    };

    // Find the Pod resource whose output address matches the backend address.
    // This gives us the pod name and the node it's running on.
    use futures::TryStreamExt;
    let mut resources = ns
        .list_resources()
        .await
        .map_err(|e| anyhow!("failed to list resources: {e}"))?;

    while let Some(resource) = resources.try_next().await.map_err(|e| anyhow!("{e}"))? {
        if resource.resource_type != POD_TYPE {
            continue;
        }
        let outputs = match &resource.outputs {
            Some(o) => o,
            None => continue,
        };
        if let sclc::Value::Str(addr) = outputs.get("address")
            && addr == &address
        {
            let node_name = match outputs.get("node") {
                sclc::Value::Str(s) => s.clone(),
                _ => continue,
            };
            tracing::info!(
                backend_address = %address,
                pod_name = %resource.name,
                node = %node_name,
                "resolved Host.Port backend to pod"
            );
            return Ok((resource.name, node_name, port, protocol));
        }
    }

    Err(anyhow!(
        "no Pod resource found with address {address} for Host.Port backend"
    ))
}

/// Handle a port-forward session: resolve the resource QID, connect to SCOC,
/// and proxy data between the SSH channel and the gRPC stream.
pub(crate) async fn handle_port_forward(
    user: &udb::User,
    resource_qid_str: &str,
    channel: &mut Channel<server::Msg>,
    rx: &mut mpsc::Receiver<ChannelMessage>,
    rdb_client: &rdb::Client,
    udb_client: &udb::Client,
    mut node_registry_redis: redis::aio::MultiplexedConnection,
) -> anyhow::Result<()> {
    use ids::ResourceQid;
    use redis::AsyncCommands;

    // Parse the resource QID
    let resource_qid: ResourceQid = resource_qid_str
        .parse()
        .map_err(|_| UserFacingError(format!("invalid resource QID: {resource_qid_str}")))?;

    // Access check: user must be a member of the organization
    let repo_qid = resource_qid.environment_qid().repo_qid();
    ensure_repo_access(user, repo_qid, udb_client).await?;

    let resource_type = &resource_qid.resource().typ;
    if resource_type != POD_PORT_TYPE && resource_type != HOST_PORT_TYPE {
        return Err(UserFacingError(format!(
            "port-forward is only supported for {POD_PORT_TYPE} and {HOST_PORT_TYPE}, \
             got {resource_type}"
        ))
        .into());
    }

    // Look up the resource in RDB
    let env_qid = resource_qid.environment_qid().to_string();
    let ns = rdb_client.namespace(env_qid);
    let resource = ns
        .resource(
            resource_qid.resource().typ.clone(),
            resource_qid.resource().name.clone(),
        )
        .get()
        .await
        .map_err(|e| anyhow!("failed to query resource: {e}"))?
        .ok_or_else(|| UserFacingError(format!("resource not found: {resource_qid}")))?;

    // Extract port-forward target info from resource inputs
    let inputs = resource
        .inputs
        .ok_or_else(|| anyhow!("resource has no inputs"))?;

    // Resolve the target pod, node, port, and protocol.
    // For Pod.Port, these come directly from inputs.
    // For Host.Port, we pick the first backend and look up the corresponding Pod resource.
    let (pod_name, node_name, port, protocol) = if resource_type == HOST_PORT_TYPE {
        resolve_host_port_target(&inputs, &ns).await?
    } else {
        resolve_pod_port_target(&inputs)?
    };

    tracing::info!(
        resource_qid = %resource_qid,
        pod_name = %pod_name,
        node = %node_name,
        port = %port,
        "resolving port-forward target"
    );

    // Look up the SCOC conduit address from the node registry
    let node_key = format!("n:{node_name}");
    let node_json: String = node_registry_redis
        .get(&node_key)
        .await
        .map_err(|e| anyhow!("failed to look up node '{node_name}' in registry: {e}"))?;
    let node_data: serde_json::Value = serde_json::from_str(&node_json)
        .map_err(|e| anyhow!("failed to parse node registry data: {e}"))?;
    let conduit_address = node_data["address"]
        .as_str()
        .ok_or_else(|| anyhow!("node '{node_name}' has no conduit address"))?;

    tracing::info!(
        conduit_address = %conduit_address,
        "connecting to SCOC conduit"
    );

    // Connect to SCOC and initiate port-forward
    let mut conduit = scop::ConduitClient::connect(conduit_address.to_string())
        .await
        .map_err(|e| anyhow!("failed to connect to SCOC at {conduit_address}: {e}"))?;

    let (grpc_tx, grpc_rx) = mpsc::channel::<scop::PortForwardRequest>(32);

    // Send the init message
    grpc_tx
        .send(scop::PortForwardRequest {
            payload: Some(scop::PortForwardPayload::Init(scop::PortForwardInit {
                pod_name,
                port,
                protocol,
            })),
        })
        .await
        .map_err(|e| anyhow!("failed to send init: {e}"))?;

    let response_stream = conduit
        .port_forward(tokio_stream::wrappers::ReceiverStream::new(grpc_rx))
        .await
        .map_err(|e| UserFacingError(format!("port-forward failed: {e}")))?
        .into_inner();

    tracing::info!("port-forward session established");

    // Proxy data bidirectionally:
    // SSH channel data -> gRPC request stream
    // gRPC response stream -> SSH channel data
    let mut response_stream = response_stream;

    // Task: gRPC responses -> SSH channel
    let grpc_to_ssh = async {
        use futures::TryStreamExt;
        while let Some(response) = response_stream.try_next().await? {
            if !response.data.is_empty() {
                channel.data(&response.data[..]).await?;
            }
        }
        Ok::<(), anyhow::Error>(())
    };

    // Task: SSH channel data -> gRPC requests
    let ssh_to_grpc = async {
        loop {
            match rx.recv().await {
                Some(ChannelMessage::Data(data)) => {
                    if grpc_tx
                        .send(scop::PortForwardRequest {
                            payload: Some(scop::PortForwardPayload::Data(data)),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(ChannelMessage::Eof) | None => break,
                Some(ChannelMessage::Command(_)) => {
                    // Ignore unexpected command messages during port-forward
                }
            }
        }
        // Drop the sender to signal the gRPC stream is done
        drop(grpc_tx);
        Ok::<(), anyhow::Error>(())
    };

    // Run both directions concurrently; finish when either side closes
    tokio::select! {
        result = grpc_to_ssh => {
            if let Err(e) = result {
                tracing::debug!("gRPC->SSH ended: {e}");
            }
        }
        result = ssh_to_grpc => {
            if let Err(e) = result {
                tracing::debug!("SSH->gRPC ended: {e}");
            }
        }
    }

    Ok(())
}

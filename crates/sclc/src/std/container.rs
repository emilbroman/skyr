//! Std/Container - Container orchestration resources
//!
//! This module provides Image, Pod, Port, Attachment, Host, and Host.Port resources
//! for container orchestration and networking.
//!
//! Resource types:
//! - `Std/Container.Image` - Container image build via BuildKit
//! - `Std/Container.Pod` - Pod sandbox with inline containers
//! - `Std/Container.Pod.Port` - Pod port (firewall opening)
//! - `Std/Container.Pod.Attachment` - Egress attachment from pod to a port
//! - `Std/Container.Host` - Virtual load balancer with DNS name and VIP
//! - `Std/Container.Host.Port` - Load-balanced port routing to backend pod ports
//! - `Std/Container.Host.InternetAddress` - Public internet exposure via floating IP

use std::hash::{DefaultHasher, Hash, Hasher};

use ids::ResourceId;
use sha1::Digest;

use crate::{EvalCtx, ExternFnValue, Record, TrackedValue, Value, ValueAssertions};

const IMAGE_RESOURCE_TYPE: &str = "Std/Container.Image";
const POD_RESOURCE_TYPE: &str = "Std/Container.Pod";
const PORT_RESOURCE_TYPE: &str = "Std/Container.Pod.Port";
const ATTACHMENT_RESOURCE_TYPE: &str = "Std/Container.Pod.Attachment";
const HOST_RESOURCE_TYPE: &str = "Std/Container.Host";
const HOST_PORT_RESOURCE_TYPE: &str = "Std/Container.Host.Port";
const HOST_INTERNET_ADDRESS_RESOURCE_TYPE: &str = "Std/Container.Host.InternetAddress";

pub fn register_extern<S: crate::SourceRepo>(eval: &mut crate::Eval<'_, S>) {
    eval.add_extern_fn(IMAGE_RESOURCE_TYPE, image_extern_fn);
    eval.add_extern_fn(POD_RESOURCE_TYPE, pod_extern_fn);
    eval.add_extern_fn(HOST_RESOURCE_TYPE, host_extern_fn);
}

/// Extern function for building container images via BuildKit.
///
/// Input: `{ name: Str, context: Path, containerfile: Path }`
/// Output: `{ fullname: Str, digest: Str }`
fn image_extern_fn(
    args: Vec<TrackedValue>,
    eval_ctx: &EvalCtx,
) -> Result<TrackedValue, crate::EvalError> {
    let mut args = args.into_iter();
    let config_arg = args
        .next()
        .unwrap_or_else(|| TrackedValue::new(Value::Nil));
    let argument_dependencies = config_arg.dependencies.clone();

    if config_arg.value.has_pending() {
        return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
    }

    let config = config_arg.value.assert_record()?;

    // Extract inputs
    let name = config.get("name").assert_str_ref()?.to_owned();
    let context = config.get("context").assert_path_ref()?.clone();
    let containerfile = config.get("containerfile").assert_path_ref()?.clone();

    // Compute content-addressed resource name from Git object hashes
    let mut hasher = sha1::Sha1::new();
    hasher.update(context.hash.to_hex().to_string().as_bytes());
    hasher.update(containerfile.hash.to_hex().to_string().as_bytes());
    let hash = hex::encode(hasher.finalize());
    let resource_name = format!("{name}-{hash}");

    let resource_id = ResourceId {
        typ: IMAGE_RESOURCE_TYPE.to_string(),
        name: resource_name.clone(),
    };

    // Build inputs for the RTP plugin
    let mut inputs = Record::default();
    inputs.insert(String::from("name"), Value::Str(name));
    inputs.insert(String::from("context"), Value::Path(context));
    inputs.insert(String::from("containerfile"), Value::Path(containerfile));

    let Some(outputs) = eval_ctx.resource(
        IMAGE_RESOURCE_TYPE,
        &resource_name,
        &inputs,
        argument_dependencies.clone(),
    )?
    else {
        // Resource is pending
        return Ok(TrackedValue::pending().with_dependency(resource_id));
    };

    // Extract outputs from the plugin
    let fullname = outputs
        .get("fullname")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();
    let digest = outputs
        .get("digest")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();

    // Build the result record
    let mut result = Record::default();
    result.insert(String::from("fullname"), Value::Str(fullname));
    result.insert(String::from("digest"), Value::Str(digest));

    Ok(TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
}

/// Compute a deterministic hash of the pod inputs for the resource name.
///
/// Builds a canonical JSON representation of the inputs (sorted keys via BTreeMap)
/// and hashes it to produce a hex suffix for the resource name.
fn compute_inputs_hash(
    name: &str,
    containers: &[serde_json::Value],
    env: &serde_json::Value,
) -> String {
    let canonical = serde_json::json!({
        "containers": containers,
        "env": env,
        "name": name,
    });
    let json_str = serde_json::to_string(&canonical).unwrap();
    let mut hasher = DefaultHasher::new();
    Hash::hash(&json_str, &mut hasher);
    format!("{:x}", hasher.finish())
}

/// Convert a `Value::Dict` with string keys/values to a sorted JSON object for hashing.
fn dict_to_sorted_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Dict(dict) => {
            let mut map = serde_json::Map::new();
            for (k, v) in dict.iter() {
                if let (Value::Str(key), Value::Str(val)) = (k, v) {
                    map.insert(key.clone(), serde_json::Value::String(val.clone()));
                }
            }
            // Sort by key for deterministic hashing
            let sorted: std::collections::BTreeMap<String, serde_json::Value> =
                map.into_iter().collect();
            serde_json::json!(sorted)
        }
        _ => serde_json::Value::Null,
    }
}

/// Extern function for creating Pod resources.
///
/// Input: `{ name: Str, containers: [{ image: Str, env: #{Str: Str}? }], env: #{Str: Str}? }`
/// Output: `{ name: Str, node: Str, address: Str, Port: fn(...), Attachment: fn(...) }`
fn pod_extern_fn(
    args: Vec<TrackedValue>,
    eval_ctx: &EvalCtx,
) -> Result<TrackedValue, crate::EvalError> {
    let mut args = args.into_iter();
    let config_arg = args
        .next()
        .unwrap_or_else(|| TrackedValue::new(Value::Nil));
    let argument_dependencies = config_arg.dependencies.clone();

    if config_arg.value.has_pending() {
        return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
    }

    let config = config_arg.value.assert_record()?;

    // Extract inputs
    let name = config.get("name").assert_str_ref()?.to_owned();
    let containers_value = config.get("containers").clone();
    let env_value = config.get("env").clone();

    // Extract container list for building inputs and computing hash
    let containers_list = match &containers_value {
        Value::List(list) => list.clone(),
        _ => vec![],
    };

    // Build canonical JSON representation of containers for hashing
    // Include container-level env in the hash so changes trigger recreation
    let containers_json: Vec<serde_json::Value> = containers_list
        .iter()
        .map(|c| {
            if let Value::Record(rec) = c {
                let image = rec
                    .get("image")
                    .assert_str_ref()
                    .unwrap_or("")
                    .to_owned();
                let env = dict_to_sorted_json(rec.get("env"));
                serde_json::json!({ "env": env, "image": image })
            } else {
                serde_json::json!(null)
            }
        })
        .collect();

    // Build canonical JSON for pod-level env
    let env_json = dict_to_sorted_json(&env_value);

    // Compute resource name: "{name}-{hash}"
    let hash = compute_inputs_hash(&name, &containers_json, &env_json);
    let resource_name = format!("{name}-{hash}");

    let resource_id = ResourceId {
        typ: POD_RESOURCE_TYPE.to_string(),
        name: resource_name.clone(),
    };

    // Build inputs for the RTP plugin
    let mut inputs = Record::default();
    inputs.insert(String::from("name"), Value::Str(resource_name.clone()));
    inputs.insert(String::from("containers"), containers_value);
    inputs.insert(String::from("env"), env_value);

    let Some(outputs) = eval_ctx.resource(
        POD_RESOURCE_TYPE,
        &resource_name,
        &inputs,
        argument_dependencies.clone(),
    )?
    else {
        // Resource is pending
        return Ok(TrackedValue::pending().with_dependency(resource_id));
    };

    // Extract outputs from the plugin
    let node = outputs
        .get("node")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();
    let address = outputs
        .get("address")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();

    // Build the result record
    let mut result = Record::default();
    result.insert(String::from("name"), Value::Str(resource_name.clone()));
    result.insert(String::from("node"), Value::Str(node.clone()));
    result.insert(String::from("address"), Value::Str(address.clone()));

    // Create the Port function
    let port_fn = create_port_fn(
        resource_name.clone(),
        address.clone(),
        node.clone(),
        resource_id.clone(),
    );
    result.insert(String::from("Port"), Value::ExternFn(port_fn));

    // Create the Attachment function
    let attachment_fn = create_attachment_fn(
        resource_name,
        address,
        node,
        resource_id.clone(),
    );
    result.insert(String::from("Attachment"), Value::ExternFn(attachment_fn));

    Ok(TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
}

/// Creates an ExternFnValue for exposing ports on a pod.
///
/// The returned function captures the pod's context (resource_name, address, node)
/// and uses them when creating Pod.Port resources.
fn create_port_fn(
    resource_name: String,
    pod_address: String,
    node: String,
    pod_resource_id: ResourceId,
) -> ExternFnValue {
    ExternFnValue::new(Box::new(move |args: Vec<TrackedValue>, eval_ctx: &EvalCtx| {
        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| TrackedValue::new(Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        // The port depends on the pod
        argument_dependencies.insert(pod_resource_id.clone());

        if config_arg.value.has_pending() {
            return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
        }

        let config = config_arg.value.assert_record()?;

        // Extract port-specific inputs
        let port = *config.get("port").assert_int_ref()?;
        let protocol = config
            .get("protocol")
            .assert_str_ref()
            .unwrap_or("tcp")
            .to_lowercase();

        // Build the resource ID: "{resource_name}:{port}/{protocol}"
        let resource_id_str = format!("{}:{}/{}", resource_name, port, protocol);
        let resource_id = ResourceId {
            typ: PORT_RESOURCE_TYPE.to_string(),
            name: resource_id_str.clone(),
        };

        // Build inputs for the RTP plugin
        let mut inputs = Record::default();
        inputs.insert(String::from("podName"), Value::Str(resource_name.clone()));
        inputs.insert(String::from("ip"), Value::Str(pod_address.clone()));
        inputs.insert(String::from("node"), Value::Str(node.clone()));
        inputs.insert(String::from("port"), Value::Int(port));
        inputs.insert(String::from("protocol"), Value::Str(protocol.clone()));

        let Some(_outputs) = eval_ctx.resource(
            PORT_RESOURCE_TYPE,
            &resource_id_str,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            // Resource is pending
            return Ok(TrackedValue::pending().with_dependency(resource_id));
        };

        // Build the result record from closure/inputs (plugin outputs are empty)
        let mut result = Record::default();
        result.insert(String::from("address"), Value::Str(pod_address.clone()));
        result.insert(String::from("port"), Value::Int(port));
        result.insert(String::from("protocol"), Value::Str(protocol));

        Ok(TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
    }))
}

/// Creates an ExternFnValue for creating egress attachments from a pod to a port.
///
/// The returned function takes a Port record `{ address, port, protocol }` and
/// creates an Attachment resource granting egress access from the pod.
fn create_attachment_fn(
    resource_name: String,
    pod_address: String,
    node: String,
    pod_resource_id: ResourceId,
) -> ExternFnValue {
    ExternFnValue::new(Box::new(move |args: Vec<TrackedValue>, eval_ctx: &EvalCtx| {
        let mut args = args.into_iter();
        let port_arg = args
            .next()
            .unwrap_or_else(|| TrackedValue::new(Value::Nil));
        let mut argument_dependencies = port_arg.dependencies.clone();

        // The attachment depends on the pod
        argument_dependencies.insert(pod_resource_id.clone());

        if port_arg.value.has_pending() {
            return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
        }

        let port_record = port_arg.value.assert_record()?;

        // Extract destination port details
        let dest_address = port_record.get("address").assert_str_ref()?.to_owned();
        let port = *port_record.get("port").assert_int_ref()?;
        let protocol = port_record.get("protocol").assert_str_ref()?.to_owned();

        // Build the resource ID: "{resource_name}@{dest_address}:{port}/{protocol}"
        let resource_id_str = format!("{}@{}:{}/{}", resource_name, dest_address, port, protocol);
        let resource_id = ResourceId {
            typ: ATTACHMENT_RESOURCE_TYPE.to_string(),
            name: resource_id_str.clone(),
        };

        // Build inputs for the RTP plugin
        let mut inputs = Record::default();
        inputs.insert(String::from("podName"), Value::Str(resource_name.clone()));
        inputs.insert(String::from("node"), Value::Str(node.clone()));
        inputs.insert(String::from("source"), Value::Str(pod_address.clone()));
        inputs.insert(String::from("destination"), Value::Str(dest_address.clone()));
        inputs.insert(String::from("port"), Value::Int(port));
        inputs.insert(String::from("protocol"), Value::Str(protocol.clone()));

        let Some(_outputs) = eval_ctx.resource(
            ATTACHMENT_RESOURCE_TYPE,
            &resource_id_str,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            // Resource is pending
            return Ok(TrackedValue::pending().with_dependency(resource_id));
        };

        // Build the result record
        let mut port_result = Record::default();
        port_result.insert(String::from("address"), Value::Str(dest_address));
        port_result.insert(String::from("port"), Value::Int(port));
        port_result.insert(String::from("protocol"), Value::Str(protocol));

        let mut result = Record::default();
        result.insert(String::from("port"), Value::Record(port_result));
        result.insert(String::from("clientAddress"), Value::Str(pod_address.clone()));

        Ok(TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
    }))
}

/// Extern function for creating Host resources (virtual load balancer with DNS).
///
/// Input: `{ name: Str }`
/// Output: `{ hostname: Str, Port: fn(...) }`
fn host_extern_fn(
    args: Vec<TrackedValue>,
    eval_ctx: &EvalCtx,
) -> Result<TrackedValue, crate::EvalError> {
    let mut args = args.into_iter();
    let config_arg = args
        .next()
        .unwrap_or_else(|| TrackedValue::new(Value::Nil));
    let argument_dependencies = config_arg.dependencies.clone();

    if config_arg.value.has_pending() {
        return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
    }

    let config = config_arg.value.assert_record()?;

    // Extract the name from input
    let name = config.get("name").assert_str_ref()?.to_owned();

    let resource_id = ResourceId {
        typ: HOST_RESOURCE_TYPE.to_string(),
        name: name.clone(),
    };

    // Build inputs for the RTP plugin
    let mut inputs = Record::default();
    inputs.insert(String::from("name"), Value::Str(name.clone()));

    let Some(outputs) = eval_ctx.resource(
        HOST_RESOURCE_TYPE,
        &name,
        &inputs,
        argument_dependencies.clone(),
    )?
    else {
        // Resource is pending
        return Ok(TrackedValue::pending().with_dependency(resource_id));
    };

    // Extract outputs from the plugin
    let hostname = outputs
        .get("hostname")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();
    let vip = outputs
        .get("vip")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();

    // Build the result record with outputs
    let mut result = Record::default();
    result.insert(String::from("hostname"), Value::Str(hostname.clone()));

    // Create the Port function that captures the host's context
    let port_fn = create_host_port_fn(hostname.clone(), vip.clone(), resource_id.clone());
    result.insert(String::from("Port"), Value::ExternFn(port_fn));

    // Create the InternetAddress function that captures the host's context
    let internet_address_fn =
        create_host_internet_address_fn(hostname, vip, resource_id.clone());
    result.insert(
        String::from("InternetAddress"),
        Value::ExternFn(internet_address_fn),
    );

    Ok(TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
}

/// Creates an ExternFnValue for creating ports on a Host.
///
/// The returned function captures the host's context (hostname, vip)
/// and uses them when creating Host.Port resources.
fn create_host_port_fn(
    host_hostname: String,
    host_vip: String,
    host_resource_id: ResourceId,
) -> ExternFnValue {
    ExternFnValue::new(Box::new(move |args: Vec<TrackedValue>, eval_ctx: &EvalCtx| {
        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| TrackedValue::new(Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        // The host port depends on the host
        argument_dependencies.insert(host_resource_id.clone());

        if config_arg.value.has_pending() {
            return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
        }

        let config = config_arg.value.assert_record()?;

        // Extract port-specific inputs
        let port = *config.get("port").assert_int_ref()?;
        let protocol = config
            .get("protocol")
            .assert_str_ref()
            .unwrap_or("tcp")
            .to_lowercase();

        // Extract backends (list of port resource records)
        let backends_value = config.get("backends").clone();

        // Build the resource ID: "{hostname}:{port}/{protocol}"
        let resource_id_str = format!("{}:{}/{}", host_hostname, port, protocol);
        let resource_id = ResourceId {
            typ: HOST_PORT_RESOURCE_TYPE.to_string(),
            name: resource_id_str.clone(),
        };

        // Build inputs for the RTP plugin
        let mut inputs = Record::default();
        inputs.insert(String::from("hostHostname"), Value::Str(host_hostname.clone()));
        inputs.insert(String::from("hostVip"), Value::Str(host_vip.clone()));
        inputs.insert(String::from("port"), Value::Int(port));
        inputs.insert(String::from("protocol"), Value::Str(protocol.clone()));
        inputs.insert(String::from("backends"), backends_value);

        let Some(outputs) = eval_ctx.resource(
            HOST_PORT_RESOURCE_TYPE,
            &resource_id_str,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            // Resource is pending
            return Ok(TrackedValue::pending().with_dependency(resource_id));
        };

        // Extract outputs from the plugin
        let out_hostname = outputs
            .get("hostname")
            .assert_str_ref()
            .unwrap_or(&host_hostname)
            .to_owned();
        let out_address = outputs
            .get("address")
            .assert_str_ref()
            .unwrap_or(&host_vip)
            .to_owned();
        let out_port = outputs
            .get("port")
            .assert_int_ref()
            .copied()
            .unwrap_or(port);
        let out_protocol = outputs
            .get("protocol")
            .assert_str_ref()
            .unwrap_or(&protocol)
            .to_owned();

        // Build the result record.
        // Host.Port outputs include both hostname and address so that it can be:
        // 1. Used in allow lists (address + port + protocol)
        // 2. Used for DNS-based access (hostname + port)
        let mut result = Record::default();
        result.insert(String::from("hostname"), Value::Str(out_hostname));
        result.insert(String::from("address"), Value::Str(out_address));
        result.insert(String::from("port"), Value::Int(out_port));
        result.insert(String::from("protocol"), Value::Str(out_protocol));

        Ok(TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
    }))
}

/// Creates an ExternFnValue for exposing a Host on the public internet.
///
/// The returned function captures the host's context (hostname, vip)
/// and uses them when creating Host.InternetAddress resources.
fn create_host_internet_address_fn(
    host_hostname: String,
    host_vip: String,
    host_resource_id: ResourceId,
) -> ExternFnValue {
    ExternFnValue::new(Box::new(
        move |args: Vec<TrackedValue>, eval_ctx: &EvalCtx| {
            let mut args = args.into_iter();
            let config_arg = args
                .next()
                .unwrap_or_else(|| TrackedValue::new(Value::Nil));
            let mut argument_dependencies = config_arg.dependencies.clone();

            // The internet address depends on the host
            argument_dependencies.insert(host_resource_id.clone());

            if config_arg.value.has_pending() {
                return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
            }

            let config = config_arg.value.assert_record()?;

            // Extract input
            let name = config.get("name").assert_str_ref()?.to_owned();

            // Build the resource ID: "{hostname}/{name}"
            let resource_id_str = format!("{}/{}", host_hostname, name);
            let resource_id = ResourceId {
                typ: HOST_INTERNET_ADDRESS_RESOURCE_TYPE.to_string(),
                name: resource_id_str.clone(),
            };

            // Build inputs for the RTP plugin
            let mut inputs = Record::default();
            inputs.insert(
                String::from("hostHostname"),
                Value::Str(host_hostname.clone()),
            );
            inputs.insert(String::from("hostVip"), Value::Str(host_vip.clone()));
            inputs.insert(String::from("name"), Value::Str(name));

            let Some(outputs) = eval_ctx.resource(
                HOST_INTERNET_ADDRESS_RESOURCE_TYPE,
                &resource_id_str,
                &inputs,
                argument_dependencies.clone(),
            )?
            else {
                // Resource is pending
                return Ok(TrackedValue::pending().with_dependency(resource_id));
            };

            // Only expose publicIp to SCL (lanIp and node are internal)
            let public_ip = outputs
                .get("publicIp")
                .assert_str_ref()
                .unwrap_or("")
                .to_owned();

            let mut result = Record::default();
            result.insert(String::from("publicIp"), Value::Str(public_ip));

            Ok(TrackedValue::new(Value::Record(result)).with_dependency(resource_id))
        },
    ))
}

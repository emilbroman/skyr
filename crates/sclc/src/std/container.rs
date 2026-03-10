//! Std/Container - Container orchestration resources
//!
//! This module provides Image, Pod, Container, and Port resources for container orchestration.
//!
//! Resource types:
//! - `Std/Container.Image` - Container image build via BuildKit
//! - `Std/Container.Pod` - Pod sandbox lifecycle
//! - `Std/Container.Pod.Container` - Container lifecycle within a pod
//! - `Std/Container.Pod.Port` - Pod port (firewall opening / access token)

use crate::{EvalCtx, ExternFnValue, Record, ResourceId, TrackedValue, Value, ValueAssertions};

const IMAGE_RESOURCE_TYPE: &str = "Std/Container.Image";
const POD_RESOURCE_TYPE: &str = "Std/Container.Pod";
const CONTAINER_RESOURCE_TYPE: &str = "Std/Container.Pod.Container";
const PORT_RESOURCE_TYPE: &str = "Std/Container.Pod.Port";

pub fn register_extern(eval: &mut crate::Eval) {
    eval.add_extern_fn(IMAGE_RESOURCE_TYPE, image_extern_fn);
    eval.add_extern_fn(POD_RESOURCE_TYPE, pod_extern_fn);
}

/// Extern function for building container images via BuildKit.
///
/// Input: `{ name: Str, context: Str, containerfile: Str }`
/// Output: `{ fullname: Str, digest: Str }`
fn image_extern_fn(
    args: Vec<TrackedValue>,
    eval_ctx: &EvalCtx,
) -> Result<TrackedValue, crate::EvalError> {
    let mut args = args.into_iter();
    let config_arg = args
        .next()
        .unwrap_or_else(|| TrackedValue::new(Value::Nil));
    let mut argument_dependencies = config_arg.dependencies.clone();

    let config = config_arg.value.assert_record()?;

    // Extract inputs
    let name = config.get("name").assert_str_ref()?.to_owned();
    let context = config.get("context").assert_str_ref()?.to_owned();
    let containerfile = config.get("containerfile").assert_str_ref()?.to_owned();

    // The resource ID is based on the image name
    // (the plugin will compute a content-based ID from the Git tree hash)
    let resource_id = ResourceId {
        ty: IMAGE_RESOURCE_TYPE.to_string(),
        id: name.clone(),
    };

    // Build inputs for the RTP plugin
    let mut inputs = Record::default();
    inputs.insert(String::from("name"), Value::Str(name.clone()));
    inputs.insert(String::from("context"), Value::Str(context));
    inputs.insert(String::from("containerfile"), Value::Str(containerfile));
    // Pass the namespace so the plugin can fetch the Git context
    inputs.insert(
        String::from("namespace"),
        Value::Str(eval_ctx.namespace().to_owned()),
    );

    let Some(outputs) = eval_ctx.resource(
        IMAGE_RESOURCE_TYPE,
        &name,
        &inputs,
        argument_dependencies.clone(),
    )?
    else {
        // Resource is pending
        argument_dependencies.insert(resource_id);
        return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
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

    argument_dependencies.insert(resource_id);
    Ok(TrackedValue::new(Value::Record(result)).with_dependencies(argument_dependencies))
}

/// Extern function for creating Pod resources.
///
/// Input: `{ name: Str, allow: [{ address: Str, port: Int, protocol: Str }]? }`
/// Output: `{ podId: Str, node: Str, name: Str, namespace: Str, address: Str, Container: fn(...), Port: fn(...) }`
fn pod_extern_fn(
    args: Vec<TrackedValue>,
    eval_ctx: &EvalCtx,
) -> Result<TrackedValue, crate::EvalError> {
    let mut args = args.into_iter();
    let config_arg = args
        .next()
        .unwrap_or_else(|| TrackedValue::new(Value::Nil));
    let mut argument_dependencies = config_arg.dependencies.clone();

    let config = config_arg.value.assert_record()?;

    // Extract the name from input
    let name = config.get("name").assert_str_ref()?.to_owned();

    let resource_id = ResourceId {
        ty: POD_RESOURCE_TYPE.to_string(),
        id: name.clone(),
    };

    // Extract the optional allow list (list of port resource records)
    let allow_value = config.get("allow").clone();

    // Build inputs for the RTP plugin
    // The plugin expects: name, namespace, uid, node (optional), labels, annotations
    // For the minimal interface, we only pass name and generate uid/namespace
    let mut inputs = Record::default();
    inputs.insert(String::from("name"), Value::Str(name.clone()));
    // Use the deployment namespace from eval context
    inputs.insert(
        String::from("namespace"),
        Value::Str(eval_ctx.namespace().to_owned()),
    );
    // Generate a uid based on the name (the plugin may override this)
    inputs.insert(
        String::from("uid"),
        Value::Str(format!("{}-{}", eval_ctx.namespace(), name)),
    );
    // Pass the allow list through to the plugin (Value::Nil if not provided)
    inputs.insert(String::from("allow"), allow_value);

    let Some(outputs) = eval_ctx.resource(
        POD_RESOURCE_TYPE,
        &name,
        &inputs,
        argument_dependencies.clone(),
    )?
    else {
        // Resource is pending - return pending value with dependency
        argument_dependencies.insert(resource_id);
        return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
    };

    // Extract outputs from the plugin
    let pod_id = outputs
        .get("podId")
        .assert_str_ref()
        .unwrap_or(&name)
        .to_owned();
    let node = outputs
        .get("node")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();
    let namespace = outputs
        .get("namespace")
        .assert_str_ref()
        .unwrap_or(eval_ctx.namespace())
        .to_owned();
    let address = outputs
        .get("address")
        .assert_str_ref()
        .unwrap_or("")
        .to_owned();

    // Build the result record with outputs
    let mut result = Record::default();
    result.insert(String::from("podId"), Value::Str(pod_id.clone()));
    result.insert(String::from("node"), Value::Str(node.clone()));
    result.insert(String::from("name"), Value::Str(name.clone()));
    result.insert(String::from("namespace"), Value::Str(namespace.clone()));
    result.insert(String::from("address"), Value::Str(address.clone()));

    // Create the Container function that captures the pod's context
    let container_fn = create_container_fn(
        pod_id.clone(),
        name.clone(),
        namespace.clone(),
        node.clone(),
        resource_id.clone(),
    );
    result.insert(String::from("Container"), Value::ExternFn(container_fn));

    // Create the Port function that captures the pod's context
    let port_fn = create_port_fn(
        pod_id,
        name.clone(),
        namespace,
        address,
        node,
        resource_id.clone(),
    );
    result.insert(String::from("Port"), Value::ExternFn(port_fn));

    argument_dependencies.insert(resource_id);
    Ok(TrackedValue::new(Value::Record(result)).with_dependencies(argument_dependencies))
}

/// Creates an ExternFnValue for creating containers within a pod.
///
/// The returned function captures the pod's context (podId, name, namespace, node)
/// and uses them when creating Container resources.
fn create_container_fn(
    pod_id: String,
    pod_name: String,
    pod_namespace: String,
    node: String,
    pod_resource_id: ResourceId,
) -> ExternFnValue {
    ExternFnValue::new(Box::new(move |args: Vec<TrackedValue>, eval_ctx: &EvalCtx| {
        let mut args = args.into_iter();
        let config_arg = args
            .next()
            .unwrap_or_else(|| TrackedValue::new(Value::Nil));
        let mut argument_dependencies = config_arg.dependencies.clone();

        // The container depends on the pod
        argument_dependencies.insert(pod_resource_id.clone());

        let config = config_arg.value.assert_record()?;

        // Extract container-specific inputs
        let container_name = config.get("name").assert_str_ref()?.to_owned();
        let image = config.get("image").assert_str_ref()?.to_owned();

        // Build the resource ID for this container
        // Use pod_name:container_name as the unique ID
        let resource_id_str = format!("{}:{}", pod_name, container_name);
        let resource_id = ResourceId {
            ty: CONTAINER_RESOURCE_TYPE.to_string(),
            id: resource_id_str.clone(),
        };

        // Build inputs for the RTP plugin
        // The plugin expects: podId, podName, podNamespace, podUid, node, name, image, ...
        let mut inputs = Record::default();
        inputs.insert(String::from("podId"), Value::Str(pod_id.clone()));
        inputs.insert(String::from("podName"), Value::Str(pod_name.clone()));
        inputs.insert(String::from("podNamespace"), Value::Str(pod_namespace.clone()));
        inputs.insert(
            String::from("podUid"),
            Value::Str(format!("{}-{}", pod_namespace, pod_name)),
        );
        inputs.insert(String::from("node"), Value::Str(node.clone()));
        inputs.insert(String::from("name"), Value::Str(container_name.clone()));
        inputs.insert(String::from("image"), Value::Str(image.clone()));

        let Some(outputs) = eval_ctx.resource(
            CONTAINER_RESOURCE_TYPE,
            &resource_id_str,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            // Resource is pending
            argument_dependencies.insert(resource_id);
            return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        // Extract outputs from the plugin
        let container_id = outputs
            .get("containerId")
            .assert_str_ref()
            .unwrap_or(&container_name)
            .to_owned();

        // Build the result record
        let mut result = Record::default();
        result.insert(String::from("containerId"), Value::Str(container_id));
        result.insert(String::from("name"), Value::Str(container_name));
        result.insert(String::from("image"), Value::Str(image));

        argument_dependencies.insert(resource_id);
        Ok(TrackedValue::new(Value::Record(result)).with_dependencies(argument_dependencies))
    }))
}

/// Creates an ExternFnValue for exposing ports on a pod.
///
/// The returned function captures the pod's context (name, namespace, address)
/// and uses them when creating Pod.Port resources.
fn create_port_fn(
    pod_id: String,
    pod_name: String,
    pod_namespace: String,
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

        let config = config_arg.value.assert_record()?;

        // Extract port-specific inputs
        let port = *config.get("port").assert_int_ref()?;
        let protocol = config
            .get("protocol")
            .assert_str_ref()
            .unwrap_or("tcp")
            .to_lowercase();
        let port_name = config
            .get("name")
            .assert_str_ref()
            .ok()
            .map(|s| s.to_owned());

        // Build the resource ID: "{pod_name}:{port}/{protocol}"
        let resource_id_str = format!("{}:{}/{}", pod_name, port, protocol);
        let resource_id = ResourceId {
            ty: PORT_RESOURCE_TYPE.to_string(),
            id: resource_id_str.clone(),
        };

        // Build inputs for the RTP plugin
        let mut inputs = Record::default();
        inputs.insert(String::from("podId"), Value::Str(pod_id.clone()));
        inputs.insert(String::from("podName"), Value::Str(pod_name.clone()));
        inputs.insert(String::from("podNamespace"), Value::Str(pod_namespace.clone()));
        inputs.insert(String::from("podAddress"), Value::Str(pod_address.clone()));
        inputs.insert(String::from("node"), Value::Str(node.clone()));
        inputs.insert(String::from("port"), Value::Int(port));
        inputs.insert(String::from("protocol"), Value::Str(protocol.clone()));
        if let Some(ref name) = port_name {
            inputs.insert(String::from("name"), Value::Str(name.clone()));
        }

        let Some(outputs) = eval_ctx.resource(
            PORT_RESOURCE_TYPE,
            &resource_id_str,
            &inputs,
            argument_dependencies.clone(),
        )?
        else {
            // Resource is pending
            argument_dependencies.insert(resource_id);
            return Ok(TrackedValue::pending().with_dependencies(argument_dependencies));
        };

        // Extract outputs from the plugin
        let address = outputs
            .get("address")
            .assert_str_ref()
            .unwrap_or(&pod_address)
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

        // Build the result record
        let mut result = Record::default();
        result.insert(String::from("address"), Value::Str(address));
        result.insert(String::from("port"), Value::Int(out_port));
        result.insert(String::from("protocol"), Value::Str(out_protocol));

        argument_dependencies.insert(resource_id);
        Ok(TrackedValue::new(Value::Record(result)).with_dependencies(argument_dependencies))
    }))
}

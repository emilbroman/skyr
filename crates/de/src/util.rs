pub(crate) fn map_dependencies(
    environment_qid: &ids::EnvironmentQid,
    deps: Vec<ids::ResourceId>,
) -> Vec<rtq::ResourceRef> {
    deps.into_iter()
        .map(|dep| rtq::ResourceRef {
            environment_qid: environment_qid.clone(),
            resource_id: dep,
        })
        .collect()
}

pub(crate) fn resource_ref(
    environment_qid: &ids::EnvironmentQid,
    id: &ids::ResourceId,
) -> rtq::ResourceRef {
    rtq::ResourceRef {
        environment_qid: environment_qid.clone(),
        resource_id: id.clone(),
    }
}

pub(crate) fn serialize_inputs(
    id: &ids::ResourceId,
    inputs: &sclc::Record,
    context: &str,
) -> anyhow::Result<serde_json::Value> {
    serde_json::to_value(inputs).map_err(|error| {
        anyhow::anyhow!(
            "failed to encode {context} inputs for {}:{}: {error}",
            id.typ,
            id.name,
        )
    })
}

/// Parses an owner QID string and extracts its deployment ID and nonce.
pub(crate) fn extract_deployment_identity(
    owner_qid: &str,
) -> anyhow::Result<(ids::DeploymentId, ids::DeploymentNonce)> {
    let qid: ids::DeploymentQid = owner_qid
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid owner QID: {owner_qid}"))?;
    Ok((qid.deployment, qid.nonce))
}

/// Returns a short (up to 8 char) prefix of the deployment ID for log messages.
pub(crate) fn short_id(deployment_id: &str) -> &str {
    deployment_id.get(..8).unwrap_or(deployment_id)
}

pub(crate) fn diag_severity(level: sclc::DiagLevel) -> ldb::Severity {
    match level {
        sclc::DiagLevel::Error => ldb::Severity::Error,
        sclc::DiagLevel::Warning => ldb::Severity::Warning,
    }
}

pub(crate) fn resource_id_from(resource: &rdb::Resource) -> ids::ResourceId {
    ids::ResourceId {
        typ: resource.resource_type.clone(),
        name: resource.name.clone(),
    }
}

/// Enqueue an RTQ message, logging errors to both tracing and the deployment
/// log publisher. Returns `true` if the message was enqueued successfully.
pub(crate) async fn enqueue_message(
    rtq_publisher: &rtq::Publisher,
    log_publisher: &ldb::NamespacePublisher,
    message: &rtq::Message,
    context: &str,
    id: &ids::ResourceId,
) -> bool {
    if let Err(error) = rtq_publisher.enqueue(message).await {
        tracing::error!(
            resource_type = %id.typ,
            resource_name = %id.name,
            error = %error,
            "failed to publish {context} message",
        );
        log_publisher
            .error(format!("Failed to enqueue {context} {id}: {error}",))
            .await;
        return false;
    }
    true
}

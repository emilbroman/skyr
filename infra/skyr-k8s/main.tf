resource "kubernetes_namespace_v1" "this" {
  metadata {
    name = var.namespace
  }
}

locals {
  namespace = kubernetes_namespace_v1.this.metadata[0].name

  # Whether to deploy each dependency internally
  deploy_scylladb     = var.scylladb_hostname == null
  deploy_redis        = var.redis_hostname == null
  deploy_rabbitmq     = var.rabbitmq_hostname == null
  deploy_redpanda     = var.redpanda_hostname == null
  deploy_minio        = var.minio_endpoint_url == null
  deploy_oci_registry = var.oci_registry_url == null
  deploy_buildkit     = var.buildkit_addr == null
  deploy_smtp_relay   = var.ne_smtp == null

  # Resolved hostnames/URLs — internal services use K8s DNS
  scylladb_hostname  = coalesce(var.scylladb_hostname, "scylladb.${local.namespace}.svc.cluster.local")
  redis_hostname     = coalesce(var.redis_hostname, "redis.${local.namespace}.svc.cluster.local")
  rabbitmq_hostname  = coalesce(var.rabbitmq_hostname, "rabbitmq.${local.namespace}.svc.cluster.local")
  redpanda_hostname  = coalesce(var.redpanda_hostname, "redpanda.${local.namespace}.svc.cluster.local")
  minio_endpoint     = coalesce(var.minio_endpoint_url, "http://minio.${local.namespace}.svc.cluster.local:9000")
  minio_external_url = coalesce(var.minio_external_url, local.minio_endpoint)
  oci_registry_url   = coalesce(var.oci_registry_url, "http://oci-registry.${local.namespace}.svc.cluster.local:5000")
  buildkit_addr      = coalesce(var.buildkit_addr, "tcp://buildkit.${local.namespace}.svc.cluster.local:1234")

  # MinIO credentials: use provided or generated
  minio_access_key = coalesce(var.minio_access_key_id, one(random_password.minio_access_key[*].result))
  minio_secret_key = coalesce(var.minio_secret_access_key, one(random_password.minio_secret_key[*].result))

  # NE SMTP: when no upstream is configured, point at the in-cluster
  # Postfix relay. The relay accepts plain SMTP on port 25 with no
  # auth from inside the cluster CIDR and performs direct MX delivery
  # to recipient mail servers. This is a working default for any
  # deployment without a managed upstream (SES, Mailgun, Postmark).
  ne_smtp_host          = var.ne_smtp == null ? "smtp-relay.${local.namespace}.svc.cluster.local" : var.ne_smtp.host
  ne_smtp_port          = var.ne_smtp == null ? 25 : var.ne_smtp.port
  ne_smtp_tls           = var.ne_smtp == null ? "none" : var.ne_smtp.tls
  ne_smtp_from          = var.ne_smtp == null ? "noreply@${coalesce(var.sender_domain, "${local.namespace}.local")}" : var.ne_smtp.from
  ne_smtp_username      = var.ne_smtp == null ? "" : var.ne_smtp.username
  ne_smtp_password      = var.ne_smtp == null ? "" : var.ne_smtp.password
  ne_smtp_sender_domain = element(split("@", local.ne_smtp_from), 1)

  # Common labels
  labels = {
    "app.kubernetes.io/managed-by" = "opentofu"
    "app.kubernetes.io/part-of"    = "skyr"
  }

  # mTLS between the container plugin orchestrator and SCOC conduits is
  # enabled when all three PEM variables are provided. The tls_validation
  # check rejects any partial combination.
  scop_tls_parts_present = length(compact([
    var.scop_tls_ca_pem != null ? "1" : "",
    var.scop_tls_cert_pem != null ? "1" : "",
    var.scop_tls_key_pem != null ? "1" : "",
  ]))
  scop_tls_enabled = local.scop_tls_parts_present == 3
}

check "scop_tls_validation" {
  assert {
    condition     = local.scop_tls_parts_present == 0 || local.scop_tls_parts_present == 3
    error_message = "scop_tls_ca_pem, scop_tls_cert_pem, and scop_tls_key_pem must all be provided together, or all omitted."
  }
}

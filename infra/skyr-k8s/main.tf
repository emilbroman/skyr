resource "kubernetes_namespace" "this" {
  metadata {
    name = var.namespace
  }
}

locals {
  namespace = kubernetes_namespace.this.metadata[0].name

  # Whether to deploy each dependency internally
  deploy_scylladb     = var.scylladb_hostname == null
  deploy_redis        = var.redis_hostname == null
  deploy_rabbitmq     = var.rabbitmq_hostname == null
  deploy_redpanda     = var.redpanda_hostname == null
  deploy_minio        = var.minio_endpoint_url == null
  deploy_oci_registry = var.oci_registry_url == null
  deploy_buildkit     = var.buildkit_addr == null
  deploy_mailhog      = var.ne_smtp == null

  # Resolved hostnames/URLs — internal services use K8s DNS
  scylladb_hostname = coalesce(var.scylladb_hostname, "scylladb.${local.namespace}.svc.cluster.local")
  redis_hostname    = coalesce(var.redis_hostname, "redis.${local.namespace}.svc.cluster.local")
  rabbitmq_hostname = coalesce(var.rabbitmq_hostname, "rabbitmq.${local.namespace}.svc.cluster.local")
  redpanda_hostname = coalesce(var.redpanda_hostname, "redpanda.${local.namespace}.svc.cluster.local")
  minio_endpoint    = coalesce(var.minio_endpoint_url, "http://minio.${local.namespace}.svc.cluster.local:9000")
  oci_registry_url  = coalesce(var.oci_registry_url, "http://oci-registry.${local.namespace}.svc.cluster.local:5000")
  buildkit_addr     = coalesce(var.buildkit_addr, "tcp://buildkit.${local.namespace}.svc.cluster.local:1234")

  # MinIO credentials: use provided or generated
  minio_access_key = coalesce(var.minio_access_key_id, one(random_password.minio_access_key[*].result))
  minio_secret_key = coalesce(var.minio_secret_access_key, one(random_password.minio_secret_key[*].result))

  # NE SMTP: when no upstream is configured, point at the in-cluster
  # MailHog instance. MailHog accepts plain SMTP on port 1025 with no
  # auth, captures every message, and exposes a web UI on 8025 — fine
  # for development and staging, not a production relay.
  ne_smtp_host     = var.ne_smtp == null ? "mailhog.${local.namespace}.svc.cluster.local" : var.ne_smtp.host
  ne_smtp_port     = var.ne_smtp == null ? 1025 : var.ne_smtp.port
  ne_smtp_tls      = var.ne_smtp == null ? "none" : var.ne_smtp.tls
  ne_smtp_from     = var.ne_smtp == null ? "skyr@${local.namespace}.local" : var.ne_smtp.from
  ne_smtp_username = var.ne_smtp == null ? "" : var.ne_smtp.username
  ne_smtp_password = var.ne_smtp == null ? "" : var.ne_smtp.password

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

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

  # Common labels
  labels = {
    "app.kubernetes.io/managed-by" = "opentofu"
    "app.kubernetes.io/part-of"    = "skyr"
  }
}

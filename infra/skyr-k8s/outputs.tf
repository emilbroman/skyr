output "namespace" {
  value       = local.namespace
  description = "The Kubernetes namespace all resources were deployed into."
}

output "web_service_name" {
  value       = kubernetes_service.web.metadata[0].name
  description = "Name of the web frontend Kubernetes Service."
}

output "api_service_name" {
  value       = kubernetes_service.api.metadata[0].name
  description = "Name of the API Kubernetes Service."
}

output "scs_service_name" {
  value       = kubernetes_service.scs.metadata[0].name
  description = "Name of the SCS (SSH) Kubernetes Service."
}

output "scylladb_hostname" {
  value       = local.scylladb_hostname
  description = "Effective ScyllaDB hostname (internal or external)."
}

output "redis_hostname" {
  value       = local.redis_hostname
  description = "Effective Redis hostname (internal or external)."
}

output "rabbitmq_hostname" {
  value       = local.rabbitmq_hostname
  description = "Effective RabbitMQ hostname (internal or external)."
}

output "redpanda_hostname" {
  value       = local.redpanda_hostname
  description = "Effective Redpanda hostname (internal or external)."
}

output "minio_endpoint" {
  value       = local.minio_endpoint
  description = "Effective MinIO/S3 endpoint URL (internal or external)."
}

output "oci_registry_url" {
  value       = local.oci_registry_url
  description = "Effective OCI registry URL (internal or external)."
}

output "buildkit_addr" {
  value       = local.buildkit_addr
  description = "Effective BuildKit address (internal or external)."
}

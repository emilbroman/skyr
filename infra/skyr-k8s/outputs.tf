output "namespace" {
  value       = local.namespace
  description = "The Kubernetes namespace all resources were deployed into."
}

output "web_service_name" {
  value       = kubernetes_service_v1.web.metadata[0].name
  description = "Name of the web frontend Kubernetes Service."
}

output "api_service_name" {
  value       = kubernetes_service_v1.api.metadata[0].name
  description = "Name of the API Kubernetes Service."
}

output "scs_service_name" {
  value       = kubernetes_service_v1.scs.metadata[0].name
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

output "minio_external_endpoint" {
  value       = local.minio_external_endpoint
  description = "Public-facing MinIO/S3 base URL used for ADB presigned URLs. Falls back to minio_endpoint when minio_external_endpoint_url is unset."
}

output "oci_registry_url" {
  value       = local.oci_registry_url
  description = "Effective OCI registry URL (internal or external)."
}

output "buildkit_addr" {
  value       = local.buildkit_addr
  description = "Effective BuildKit address (internal or external)."
}

# --- NodePort outputs (populated only when service type is NodePort) ---

output "orchestrator_node_port" {
  value       = try(kubernetes_service_v1.plugin_std_container.spec[0].port[0].node_port, null)
  description = "NodePort for the container orchestrator (port 50053), if exposed."
}

output "ldb_node_port" {
  value       = try(kubernetes_service_v1.redpanda[0].spec[0].port[0].node_port, null)
  description = "NodePort for the LDB broker / Redpanda internal listener (port 9092), if exposed."
}

output "ldb_external_node_port" {
  value       = var.redpanda_advertise_host != null ? try(kubernetes_service_v1.redpanda[0].spec[0].port[1].node_port, null) : null
  description = "NodePort for the LDB broker / Redpanda external listener (port 19092), if exposed."
}

output "oci_registry_node_port" {
  value       = try(kubernetes_service_v1.oci_registry[0].spec[0].port[0].node_port, null)
  description = "NodePort for the OCI registry (port 5000), if exposed."
}

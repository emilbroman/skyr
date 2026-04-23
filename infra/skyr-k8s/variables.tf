# --- Namespace ---

variable "namespace" {
  type        = string
  description = "Kubernetes namespace for all resources."
}

# --- Image ---

variable "image_pull_policy" {
  type        = string
  description = "Image pull policy for Skyr pods."
  default     = "IfNotPresent"
}

# --- External Service Connection Strings ---
# When null (the default), the module deploys the service internally.

variable "scylladb_hostname" {
  type        = string
  description = "External ScyllaDB hostname. When null, deploys ScyllaDB internally."
  default     = null
}

variable "redis_hostname" {
  type        = string
  description = "External Redis hostname. When null, deploys Redis internally."
  default     = null
}

variable "rabbitmq_hostname" {
  type        = string
  description = "External RabbitMQ hostname. When null, deploys RabbitMQ internally."
  default     = null
}

variable "redpanda_hostname" {
  type        = string
  description = "External Redpanda/Kafka hostname. When null, deploys Redpanda internally."
  default     = null
}

variable "redpanda_advertise_host" {
  type        = string
  description = "External hostname to advertise for the Redpanda Kafka listener. When set, a second listener is added on port 19092 that advertises as this host with the NodePort. Requires ldb_service_type = NodePort."
  default     = null
}

variable "minio_endpoint_url" {
  type        = string
  description = "External MinIO/S3 endpoint URL (e.g. https://s3.amazonaws.com). When null, deploys MinIO internally."
  default     = null
}

variable "minio_bucket" {
  type        = string
  description = "S3 bucket name for artifacts."
  default     = "skyr-artifacts"
}

variable "minio_access_key_id" {
  type        = string
  description = "S3 access key ID. When null and deploying MinIO internally, credentials are generated."
  default     = null
  sensitive   = true
}

variable "minio_secret_access_key" {
  type        = string
  description = "S3 secret access key. When null and deploying MinIO internally, credentials are generated."
  default     = null
  sensitive   = true
}

variable "minio_region" {
  type        = string
  description = "S3 region."
  default     = "us-east-1"
}

variable "oci_registry_url" {
  type        = string
  description = "External OCI registry URL (e.g. https://registry.example.com). When null, deploys a registry internally."
  default     = null
}

variable "buildkit_addr" {
  type        = string
  description = "External BuildKit address (e.g. tcp://buildkit.example.com:1234). When null, deploys BuildKit internally."
  default     = null
}

variable "oci_registry_insecure" {
  type        = bool
  description = "Skip TLS verification when connecting to the OCI registry (e.g. for registries using a private CA)."
  default     = false
}

variable "oci_registry_username" {
  type        = string
  description = "Username for OCI registry basic auth. When null, no auth is configured."
  default     = null
  sensitive   = true
}

variable "oci_registry_password" {
  type        = string
  description = "Password for OCI registry basic auth. When null, no auth is configured."
  default     = null
  sensitive   = true
}

# --- DNS ---

variable "dns_zone" {
  type        = string
  description = "DNS zone served by the DNS plugin (e.g. skyr.example.com)."
  default     = "skyr.local"
}

# --- DE Scaling ---

variable "de_worker_count" {
  type        = number
  description = "Number of DE worker pods. Each gets a distinct worker index for deployment shard assignment."
  default     = 2
}

# --- RTE Scaling ---

variable "rte_worker_count" {
  type        = number
  description = "Number of RTE worker pods. Each gets a distinct worker index for shard assignment."
  default     = 3
}

variable "rte_local_workers" {
  type        = number
  description = "Number of local async workers per RTE pod."
  default     = 1
}

# --- Secrets ---

variable "scs_host_key" {
  type        = string
  description = "PEM-encoded SSH host key for the SCS git server. When null, an ED25519 key is generated."
  default     = null
  sensitive   = true
}

variable "api_challenge_salt" {
  type        = string
  description = "Challenge salt for API SSH authentication. When null, a random value is generated."
  default     = null
  sensitive   = true
}

# --- Networking ---

variable "cluster_cidr" {
  type        = string
  description = "Cluster CIDR for container plugin pod networking."
  default     = "10.42.0.0/16"
}

# --- Service Exposure ---

variable "api_service_type" {
  type        = string
  description = "Kubernetes Service type for the API (ClusterIP, LoadBalancer, or NodePort)."
  default     = "ClusterIP"
}

variable "scs_service_type" {
  type        = string
  description = "Kubernetes Service type for the SCS SSH server (ClusterIP, LoadBalancer, or NodePort)."
  default     = "ClusterIP"
}

variable "web_service_type" {
  type        = string
  description = "Kubernetes Service type for the web frontend (ClusterIP, LoadBalancer, or NodePort)."
  default     = "ClusterIP"
}

variable "orchestrator_service_type" {
  type        = string
  description = "Kubernetes Service type for the container orchestrator plugin (ClusterIP, LoadBalancer, or NodePort)."
  default     = "ClusterIP"
}

variable "ldb_service_type" {
  type        = string
  description = "Kubernetes Service type for the LDB broker / Redpanda (ClusterIP, LoadBalancer, or NodePort)."
  default     = "ClusterIP"
}

variable "oci_registry_service_type" {
  type        = string
  description = "Kubernetes Service type for the OCI registry (ClusterIP, LoadBalancer, or NodePort)."
  default     = "ClusterIP"
}

variable "dns_service_type" {
  type        = string
  description = "Kubernetes Service type for the DNS plugin (ClusterIP, LoadBalancer, or NodePort)."
  default     = "ClusterIP"
}

# --- Container plugin <-> SCOC mTLS ---
#
# Optional PEM material for mutual TLS between the container plugin
# orchestrator and SCOC conduits. All three must be provided together; leave
# all three null to run plain gRPC. The leaf certificate must carry both
# `serverAuth` and `clientAuth` Extended Key Usages.

variable "scop_tls_ca_pem" {
  type        = string
  description = "PEM-encoded CA certificate used to verify SCOC conduits and incoming orchestrator clients."
  default     = null
  sensitive   = true
}

variable "scop_tls_cert_pem" {
  type        = string
  description = "PEM-encoded leaf certificate for the container plugin orchestrator."
  default     = null
  sensitive   = true
}

variable "scop_tls_key_pem" {
  type        = string
  description = "PEM-encoded private key matching scop_tls_cert_pem."
  default     = null
  sensitive   = true
}

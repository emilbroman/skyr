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

# --- Ingress ---

variable "web_hostname" {
  type        = string
  description = "Hostname for the web frontend Traefik IngressRoute. When null, no IngressRoute is created."
  default     = null
}

variable "scs_hostname" {
  type        = string
  description = "Hostname for the SCS SSH server Traefik IngressRouteTCP. When null, no IngressRoute is created."
  default     = null
}

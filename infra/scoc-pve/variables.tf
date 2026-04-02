# --- Proxmox ---

variable "proxmox_node" {
  type        = string
  description = "Proxmox node name to create the VM on."
  default     = "pve"
}

variable "lvm_datastore_id" {
  type        = string
  description = "Proxmox datastore for VM disks."
  default     = "local-lvm"
}

variable "local_datastore_id" {
  type        = string
  description = "Proxmox datastore for cloud-init snippets (must support 'snippets' content type)."
  default     = "local"
}

variable "cloud_image_url" {
  type        = string
  description = "URL to the Alpine cloud image (qcow2)."
  default     = "https://dl-cdn.alpinelinux.org/alpine/v3.23/releases/cloud/generic_alpine-3.23.3-x86_64-bios-cloudinit-r0.qcow2"
}

# --- VM ---

variable "vm_name" {
  type        = string
  description = "Name for the SCOC worker VM."
  default     = "scoc-worker"
}

variable "vm_id" {
  type        = number
  description = "Proxmox VM ID. When null, auto-assigned."
  default     = null
}

variable "cpu_cores" {
  type        = number
  description = "Number of CPU cores for the VM."
  default     = 4
}

variable "memory_mb" {
  type        = number
  description = "Memory in MiB for the VM."
  default     = 8192
}

variable "disk_size_gb" {
  type        = number
  description = "Disk size in GiB for the VM."
  default     = 32
}

variable "network_bridge" {
  type        = string
  description = "Proxmox network bridge."
  default     = "vmbr0"
}

variable "vlan_id" {
  type        = number
  description = "VLAN tag for the network device."
  default     = 0
}

# --- Network ---

variable "vm_ip" {
  type        = string
  description = "Static IP address for the VM in CIDR notation (e.g. 192.168.1.100/24)."
}

variable "gateway" {
  type        = string
  description = "Default gateway for the VM."
}

variable "nameserver" {
  type        = string
  description = "DNS nameserver for the VM."
  default     = "1.1.1.1"
}

# --- SCOC ---

variable "node_name" {
  type        = string
  description = "SCOC node name (unique identifier for this worker)."
}

variable "orchestrator_address" {
  type        = string
  description = "Address of the container orchestrator (e.g. 192.168.1.10:30053)."
}

variable "ldb_brokers" {
  type        = string
  description = "LDB broker address (e.g. 192.168.1.10:30092)."
}

variable "oci_registry" {
  type        = string
  description = "OCI registry host:port for containerd image pulls (e.g. 192.168.1.10:30500)."
}

variable "scoc_bind" {
  type        = string
  description = "SCOC daemon bind address."
  default     = "0.0.0.0:50054"
}

variable "cpu_millis" {
  type        = number
  description = "CPU capacity to advertise (millicores)."
  default     = 4000
}

variable "memory_bytes" {
  type        = number
  description = "Memory capacity to advertise (bytes)."
  default     = 8589934592 # 8 GiB
}

variable "max_pods" {
  type        = number
  description = "Maximum number of pods this node can run."
  default     = 100
}

variable "oci_registry_insecure" {
  type        = bool
  description = "Skip TLS verification when pulling from the OCI registry (e.g. for registries using a private CA)."
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

variable "root_password" {
  type        = string
  description = "Root password for the VM. When null, password login is not configured."
  default     = null
  sensitive   = true
}

variable "ssh_authorized_keys" {
  type        = set(string)
  description = "SSH public keys to add to /root/.ssh/authorized_keys."
  default     = []
}

variable "scoc_binary_url" {
  type        = string
  description = "URL to download the SCOC binary from."
  default     = "https://github.com/emilbroman/skyr/releases/latest/download/scoc-x86_64-unknown-linux-musl"
}

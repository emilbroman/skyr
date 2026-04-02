# =============================================================================
# SCOC Worker VM on Proxmox VE
# =============================================================================

locals {
  # Strip CIDR suffix to get the bare IP for the conduit address
  vm_ip_bare = split("/", var.vm_ip)[0]
}

# --- Download Alpine cloud image ---

resource "proxmox_virtual_environment_download_file" "alpine" {
  content_type = "import"
  datastore_id = var.local_datastore_id
  node_name    = var.proxmox_node
  url          = var.cloud_image_url
}

# --- Cloud-init user data ---

resource "proxmox_virtual_environment_file" "cloud_config" {
  content_type = "snippets"
  datastore_id = var.local_datastore_id
  node_name    = var.proxmox_node

  source_raw {
    data = templatefile("${path.module}/cloud-config.yaml.tftpl", {
      node_name            = var.node_name
      scoc_bind            = var.scoc_bind
      conduit_address      = "http://${local.vm_ip_bare}:${split(":", var.scoc_bind)[1]}"
      orchestrator_address = var.orchestrator_address
      ldb_brokers          = var.ldb_brokers
      oci_registry          = var.oci_registry
      oci_registry_insecure = var.oci_registry_insecure
      oci_registry_username = var.oci_registry_username
      oci_registry_password = var.oci_registry_password
      cpu_millis            = var.cpu_millis
      memory_bytes         = var.memory_bytes
      max_pods             = var.max_pods
      scoc_binary_url      = var.scoc_binary_url
      nameserver           = var.nameserver
    })
    file_name = "${var.vm_name}-cloud-config.yaml"
  }
}

# --- VM ---

resource "proxmox_virtual_environment_vm" "scoc" {
  name      = var.vm_name
  node_name = var.proxmox_node
  vm_id     = var.vm_id

  cpu {
    cores = var.cpu_cores
  }

  memory {
    dedicated = var.memory_mb
  }

  disk {
    datastore_id = var.lvm_datastore_id
    file_id      = proxmox_virtual_environment_download_file.alpine.id
    interface    = "scsi0"
    size         = var.disk_size_gb
  }

  network_device {
    bridge  = var.network_bridge
    vlan_id = var.vlan_id
  }

  initialization {
    user_data_file_id = proxmox_virtual_environment_file.cloud_config.id

    dns {
      servers = [var.nameserver]
    }

    ip_config {
      ipv4 {
        address = var.vm_ip
        gateway = var.gateway
      }
    }
  }

  started = true

  # Don't recreate VM just because the image file changes
  lifecycle {
    ignore_changes = [disk[0].file_id]
  }
}

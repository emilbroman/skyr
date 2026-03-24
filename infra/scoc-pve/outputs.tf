output "vm_id" {
  value       = proxmox_virtual_environment_vm.scoc.vm_id
  description = "Proxmox VM ID."
}

output "vm_ip" {
  value       = local.vm_ip_bare
  description = "IP address of the SCOC worker VM."
}

output "node_name" {
  value       = var.node_name
  description = "SCOC node name."
}

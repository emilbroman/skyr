include "root" {
  path = find_in_parent_folders("root.hcl")
}

terraform {
  source = "../../skyr-k8s"
}

inputs = {
  namespace                 = "skyr"
  image_pull_policy         = "Always"
  orchestrator_service_type = "NodePort"
  ldb_service_type          = "NodePort"
  oci_registry_url          = "https://cr.bb3.internal"
  oci_registry_insecure     = true
  redpanda_advertise_host   = "node1.vm.bb3.internal"
  dns_zone                  = "host.skyr.cloud"
  cluster_cidr              = "10.100.0.0/16"
  sender_domain             = "skyr.cloud"
}

generate "proxmox_provider" {
  path      = "proxmox_versions_override.tf"
  if_exists = "overwrite_terragrunt"
  contents  = <<-EOF
    terraform {
      required_providers {
        proxmox = {
          source  = "bpg/proxmox"
          version = "~> 0.78"
        }
      }
    }
  EOF
}

generate "scoc_pve" {
  path      = "scoc-pve.tf"
  if_exists = "overwrite_terragrunt"
  contents  = <<-EOF
    variable "proxmox_api_token" {
      type        = string
      sensitive   = true
      description = "Proxmox VE API token (e.g. token-name=secret-value)."
    }

    provider "proxmox" {
      endpoint  = "https://pve.bb3.internal"
      api_token = "root@pam!$${var.proxmox_api_token}"

      ssh {
        agent    = true
        username = "root"
      }
    }

    module "scoc_pve" {
      count = 3

      source = "${get_repo_root()}/infra/scoc-pve"

      proxmox_node = "tc$${count.index + 1}"
      vm_name      = "scoc-$${count.index + 1}"
      node_name    = "scoc-$${count.index + 1}"

      vm_id   = count.index + 171
      vm_ip   = "10.20.1.$${count.index + 171}/16"
      gateway = "10.20.0.1"
      nameserver = "10.20.0.1"
      vlan_id = 4

      orchestrator_address = "http://node$${count.index + 1}.vm.bb3.internal:$${kubernetes_service_v1.plugin_std_container.spec[0].port[0].node_port}"
      ldb_brokers          = "node$${count.index + 1}.vm.bb3.internal:$${kubernetes_service_v1.redpanda[0].spec[0].port[1].node_port}"
      oci_registry          = "cr.bb3.internal"
      oci_registry_insecure = true
      oci_registry_username = var.oci_registry_username
      oci_registry_password = var.oci_registry_password
    }

    # The oci_registry_username/password variables are declared in the
    # skyr-k8s module and shared with the scoc-pve module above.
  EOF
}

generate "ingress" {
  path      = "ingress.tf"
  if_exists = "overwrite_terragrunt"
  contents  = <<-EOF
    resource "kubernetes_manifest" "certificate" {
      manifest = {
        apiVersion = "cert-manager.io/v1"
        kind       = "Certificate"
        metadata = {
          name      = "skyr-tls"
          namespace = "skyr"
        }
        spec = {
          secretName = "skyr-tls"
          dnsNames   = ["skyr.cloud"]
          issuerRef = {
            name = "acme-lets-encrypt"
            kind = "ClusterIssuer"
          }
        }
      }
    }

    resource "kubernetes_manifest" "ingress_route_api" {
      manifest = {
        apiVersion = "traefik.io/v1alpha1"
        kind       = "IngressRoute"
        metadata = {
          name      = "skyr-api"
          namespace = "skyr"
          annotations = {
            "kubernetes.io/ingress.class" = "skyr"
          }
        }
        spec = {
          entryPoints = ["websecure"]
          routes = [{
            match    = "Host(`skyr.cloud`) && (PathPrefix(`/graphql`) || PathPrefix(`/graphiql`))"
            kind     = "Rule"
            services = [{
              name = kubernetes_service_v1.api.metadata[0].name
              port = 8080
            }]
          }]
          tls = {
            secretName = "skyr-tls"
          }
        }
      }
    }

    resource "kubernetes_manifest" "ingress_route_web" {
      manifest = {
        apiVersion = "traefik.io/v1alpha1"
        kind       = "IngressRoute"
        metadata = {
          name      = "skyr-web"
          namespace = "skyr"
          annotations = {
            "kubernetes.io/ingress.class" = "skyr"
          }
        }
        spec = {
          entryPoints = ["websecure"]
          routes = [{
            match    = "Host(`skyr.cloud`)"
            kind     = "Rule"
            priority = 1
            services = [{
              name = kubernetes_service_v1.web.metadata[0].name
              port = 80
            }]
          }]
          tls = {
            secretName = "skyr-tls"
          }
        }
      }
    }

    resource "kubernetes_manifest" "ingress_route_tcp_scs" {
      manifest = {
        apiVersion = "traefik.io/v1alpha1"
        kind       = "IngressRouteTCP"
        metadata = {
          name      = "skyr-scs"
          namespace = "skyr"
          annotations = {
            "kubernetes.io/ingress.class" = "skyr"
          }
        }
        spec = {
          entryPoints = ["ssh"]
          routes = [{
            match = "HostSNI(`*`)"
            services = [{
              name = kubernetes_service_v1.scs.metadata[0].name
              port = 2222
            }]
          }]
        }
      }
    }

    resource "kubernetes_manifest" "ingress_route_udp_dns" {
      manifest = {
        apiVersion = "traefik.io/v1alpha1"
        kind       = "IngressRouteUDP"
        metadata = {
          name      = "skyr-dns"
          namespace = "skyr"
          annotations = {
            "kubernetes.io/ingress.class" = "skyr"
          }
        }
        spec = {
          entryPoints = ["dns"]
          routes = [{
            services = [{
              name = kubernetes_service_v1.plugin_std_dns.metadata[0].name
              port = 53
            }]
          }]
        }
      }
    }
  EOF
}

include "root" {
  path = find_in_parent_folders("root.hcl")
}

terraform {
  source = "../../skyr-k8s"
}

inputs = {
  namespace = "skyr-dev"
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
          namespace = "skyr-dev"
        }
        spec = {
          secretName = "skyr-tls"
          dnsNames   = ["dev.skyr.cloud"]
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
          namespace = "skyr-dev"
          annotations = {
            "kubernetes.io/ingress.class" = "external"
          }
        }
        spec = {
          entryPoints = ["websecure"]
          routes = [{
            match    = "Host(`dev.skyr.cloud`)"
            kind     = "Rule"
            services = [{
              name = kubernetes_service.api.metadata[0].name
              port = 8080
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
          namespace = "skyr-dev"
          annotations = {
            "kubernetes.io/ingress.class" = "external"
          }
        }
        spec = {
          entryPoints = ["ssh2"]
          routes = [{
            match = "HostSNI(`*`)"
            services = [{
              name = kubernetes_service.scs.metadata[0].name
              port = 2222
            }]
          }]
        }
      }
    }
  EOF
}

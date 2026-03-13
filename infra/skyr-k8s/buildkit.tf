# BuildKit needs a config file that references the OCI registry as insecure/HTTP.
# This is dynamically generated based on the resolved registry URL.

locals {
  # Extract host:port from the OCI registry URL for buildkitd.toml
  oci_registry_host = replace(replace(local.oci_registry_url, "http://", ""), "https://", "")

  buildkitd_toml = <<-EOT
    [registry."${local.oci_registry_host}"]
      http = true
      insecure = true
  EOT
}

resource "kubernetes_config_map" "buildkit" {
  count = local.deploy_buildkit ? 1 : 0

  metadata {
    name      = "buildkit-config"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "buildkit" })
  }

  data = {
    "buildkitd.toml" = local.buildkitd_toml
  }
}

resource "kubernetes_deployment" "buildkit" {
  count = local.deploy_buildkit ? 1 : 0

  metadata {
    name      = "buildkit"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "buildkit" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "buildkit" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "buildkit" })
      }

      spec {
        volume {
          name = "config"
          config_map {
            name = kubernetes_config_map.buildkit[0].metadata[0].name
          }
        }

        container {
          name  = "buildkit"
          image = "moby/buildkit:latest"

          args = [
            "--addr", "tcp://0.0.0.0:1234",
            "--config", "/etc/buildkit/buildkitd.toml",
          ]

          security_context {
            privileged = true
          }

          volume_mount {
            name       = "config"
            mount_path = "/etc/buildkit"
            read_only  = true
          }

          port {
            container_port = 1234
            protocol       = "TCP"
          }
        }
      }
    }
  }
}

resource "kubernetes_service" "buildkit" {
  count = local.deploy_buildkit ? 1 : 0

  metadata {
    name      = "buildkit"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "buildkit" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "buildkit" }

    port {
      port        = 1234
      target_port = 1234
      protocol    = "TCP"
    }
  }
}

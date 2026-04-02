# BuildKit needs a config file that references the OCI registry.
# For plain-HTTP registries, http=true is set. For HTTPS registries with
# untrusted CAs (e.g. private CA), insecure=true skips TLS verification.

locals {
  # Extract host:port from the OCI registry URL for buildkitd.toml
  oci_registry_host = replace(replace(local.oci_registry_url, "http://", ""), "https://", "")
  oci_registry_http = startswith(local.oci_registry_url, "http://")

  oci_registry_has_auth = var.oci_registry_username != null && var.oci_registry_password != null

  buildkitd_toml = <<-EOT
    [registry."${local.oci_registry_host}"]
    %{if local.oci_registry_http~}
      http = true
      insecure = true
    %{endif~}
    %{if !local.oci_registry_http && var.oci_registry_insecure~}
      insecure = true
    %{endif~}
  EOT

  # Docker config.json for registry basic auth (used by BuildKit to push)
  docker_config_json = local.oci_registry_has_auth ? jsonencode({
    auths = {
      (local.oci_registry_host) = {
        auth = base64encode("${var.oci_registry_username}:${var.oci_registry_password}")
      }
    }
  }) : null
}

resource "kubernetes_secret" "oci_registry_auth" {
  count = local.oci_registry_has_auth ? 1 : 0

  metadata {
    name      = "oci-registry-auth"
    namespace = local.namespace
    labels    = local.labels
  }

  type = "Opaque"

  data = {
    "config.json" = local.docker_config_json
  }
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

        dynamic "volume" {
          for_each = local.oci_registry_has_auth ? [1] : []
          content {
            name = "registry-auth"
            secret {
              secret_name = kubernetes_secret.oci_registry_auth[0].metadata[0].name
            }
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

          dynamic "volume_mount" {
            for_each = local.oci_registry_has_auth ? [1] : []
            content {
              name       = "registry-auth"
              mount_path = "/root/.docker"
              read_only  = true
            }
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

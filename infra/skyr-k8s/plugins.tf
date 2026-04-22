# =============================================================================
# plugin-std-container — Container orchestrator (ports 50053 + 50054 gRPC)
#
# Note: The random, artifact, crypto, and time plugins run as sidecar containers
# inside the RTE pods (see services.tf), communicating via Unix sockets.
# The container and DNS plugins are standalone deployments because they have
# their own infrastructure dependencies.
# =============================================================================

resource "kubernetes_secret" "plugin_std_container_tls" {
  count = local.scop_tls_enabled ? 1 : 0

  metadata {
    name      = "plugin-std-container-tls"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-container" })
  }

  type = "kubernetes.io/tls"

  data = {
    "ca.pem"  = var.scop_tls_ca_pem
    "tls.crt" = var.scop_tls_cert_pem
    "tls.key" = var.scop_tls_key_pem
  }
}

resource "kubernetes_deployment" "plugin_std_container" {
  metadata {
    name      = "plugin-std-container"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-container" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "plugin-std-container" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-container" })
      }

      spec {
        volume {
          name = "tmp"
          empty_dir {}
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

        dynamic "volume" {
          for_each = local.scop_tls_enabled ? [1] : []
          content {
            name = "scop-tls"
            secret {
              secret_name = kubernetes_secret.plugin_std_container_tls[0].metadata[0].name
            }
          }
        }

        container {
          name              = "plugin-std-container"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_container:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_container"]
          args = concat([
            "--bind", "0.0.0.0:50053",
            # Canonical hostname used as `from_node` / `source` in outbound
            # overlay-peer gossip. This is the in-cluster Service DNS name
            # so SCOCs can attribute orchestrator-originated entries and
            # exclude the orchestrator from reactive fan-out.
            "--orchestrator-hostname", "plugin-std-container.${local.namespace}.svc.cluster.local",
            "--rtp-bind", "tcp://0.0.0.0:50054",
            "--node-registry-hostname", local.redis_hostname,
            "--cdb-hostnames", "${local.scylladb_hostname}:9042",
            "--buildkit-addr", local.buildkit_addr,
            "--registry-url", local.oci_registry_url,
            "--ldb-hostname", local.redpanda_hostname,
            "--cluster-cidr", var.cluster_cidr,
            ],
            var.oci_registry_insecure ? ["--insecure-registry"] : [],
            local.scop_tls_enabled ? [
              "--tls-ca", "/etc/skyr/tls/ca.pem",
              "--tls-cert", "/etc/skyr/tls/tls.crt",
              "--tls-key", "/etc/skyr/tls/tls.key",
            ] : [],
          )

          dynamic "env" {
            for_each = local.oci_registry_has_auth ? [1] : []
            content {
              name  = "DOCKER_CONFIG"
              value = "/root/.docker"
            }
          }

          port {
            name           = "orchestrator"
            container_port = 50053
            protocol       = "TCP"
          }

          port {
            name           = "rtp"
            container_port = 50054
            protocol       = "TCP"
          }

          volume_mount {
            name       = "tmp"
            mount_path = "/tmp"
          }

          dynamic "volume_mount" {
            for_each = local.oci_registry_has_auth ? [1] : []
            content {
              name       = "registry-auth"
              mount_path = "/root/.docker"
              read_only  = true
            }
          }

          dynamic "volume_mount" {
            for_each = local.scop_tls_enabled ? [1] : []
            content {
              name       = "scop-tls"
              mount_path = "/etc/skyr/tls"
              read_only  = true
            }
          }
        }
      }
    }
  }

  lifecycle {
    ignore_changes = [spec[0].template[0].spec[0].container[0].image]
  }
}

resource "kubernetes_service" "plugin_std_container" {
  metadata {
    name      = "plugin-std-container"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-container" })
  }

  spec {
    type     = var.orchestrator_service_type
    selector = { "app.kubernetes.io/name" = "plugin-std-container" }

    port {
      name        = "orchestrator"
      port        = 50053
      target_port = 50053
      protocol    = "TCP"
    }

    port {
      name        = "rtp"
      port        = 50054
      target_port = 50054
      protocol    = "TCP"
    }
  }
}

# =============================================================================
# plugin-std-dns — DNS server + RTP plugin (port 50057 gRPC + 53 UDP)
#
# Standalone deployment because it needs Redis for cross-namespace DNS lookups
# and serves a UDP DNS server alongside the RTP gRPC server.
# =============================================================================

resource "kubernetes_deployment" "plugin_std_dns" {
  metadata {
    name      = "plugin-std-dns"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-dns" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "plugin-std-dns" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-dns" })
      }

      spec {
        container {
          name              = "plugin-std-dns"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_dns:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_dns"]
          args = [
            "--bind", "tcp://0.0.0.0:50057",
            "--redis-hostname", local.redis_hostname,
            "--zone", var.dns_zone,
            "--dns-port", "53",
          ]

          port {
            name           = "rtp"
            container_port = 50057
            protocol       = "TCP"
          }

          port {
            name           = "dns"
            container_port = 53
            protocol       = "UDP"
          }
        }
      }
    }
  }

  lifecycle {
    ignore_changes = [spec[0].template[0].spec[0].container[0].image]
  }
}

resource "kubernetes_service" "plugin_std_dns" {
  metadata {
    name      = "plugin-std-dns"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-dns" })
  }

  spec {
    type     = var.dns_service_type
    selector = { "app.kubernetes.io/name" = "plugin-std-dns" }

    port {
      name        = "rtp"
      port        = 50057
      target_port = 50057
      protocol    = "TCP"
    }

    port {
      name        = "dns"
      port        = 53
      target_port = 53
      protocol    = "UDP"
    }
  }
}

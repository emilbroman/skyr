# =============================================================================
# plugin-std-container — Container orchestrator (ports 50053 + 50054 gRPC)
#
# Note: The random, artifact, and crypto plugins run as sidecar containers
# inside the RTE pods (see services.tf), communicating via Unix sockets.
# The container plugin remains a standalone deployment because it has its own
# infrastructure dependencies (BuildKit, OCI registry, node registry).
# =============================================================================

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
        container {
          name              = "plugin-std-container"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_container:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_container"]
          args = [
            "--bind", "0.0.0.0:50053",
            "--rtp-bind", "tcp://0.0.0.0:50054",
            "--node-registry-hostname", local.redis_hostname,
            "--cdb-hostnames", "${local.scylladb_hostname}:9042",
            "--buildkit-addr", local.buildkit_addr,
            "--registry-url", local.oci_registry_url,
            "--ldb-hostname", local.redpanda_hostname,
            "--cluster-cidr", var.cluster_cidr,
          ]

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

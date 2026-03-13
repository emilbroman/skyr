# =============================================================================
# plugin-std-random — Random number generation (port 50051 gRPC)
# =============================================================================

resource "kubernetes_deployment" "plugin_std_random" {
  metadata {
    name      = "plugin-std-random"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-random" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "plugin-std-random" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-random" })
      }

      spec {
        container {
          name              = "plugin-std-random"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_random:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_random"]
          args    = ["--bind", "tcp://0.0.0.0:50051"]

          port {
            container_port = 50051
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

resource "kubernetes_service" "plugin_std_random" {
  metadata {
    name      = "plugin-std-random"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-random" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "plugin-std-random" }

    port {
      port        = 50051
      target_port = 50051
      protocol    = "TCP"
    }
  }
}

# =============================================================================
# plugin-std-artifact — Artifact management via S3 (port 50052 gRPC)
# =============================================================================

resource "kubernetes_deployment" "plugin_std_artifact" {
  metadata {
    name      = "plugin-std-artifact"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-artifact" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "plugin-std-artifact" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-artifact" })
      }

      spec {
        container {
          name              = "plugin-std-artifact"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_artifact:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_artifact"]
          args = [
            "--bind", "tcp://0.0.0.0:50052",
            "--adb-endpoint-url", local.minio_endpoint,
            "--adb-bucket", var.minio_bucket,
            "--adb-access-key-id", "$(MINIO_ACCESS_KEY)",
            "--adb-secret-access-key", "$(MINIO_SECRET_KEY)",
          ]

          env {
            name = "MINIO_ACCESS_KEY"
            value_from {
              secret_key_ref {
                name = kubernetes_secret.skyr.metadata[0].name
                key  = "minio-access-key-id"
              }
            }
          }

          env {
            name = "MINIO_SECRET_KEY"
            value_from {
              secret_key_ref {
                name = kubernetes_secret.skyr.metadata[0].name
                key  = "minio-secret-key"
              }
            }
          }

          port {
            container_port = 50052
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

resource "kubernetes_service" "plugin_std_artifact" {
  metadata {
    name      = "plugin-std-artifact"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-artifact" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "plugin-std-artifact" }

    port {
      port        = 50052
      target_port = 50052
      protocol    = "TCP"
    }
  }
}

# =============================================================================
# plugin-std-crypto — Cryptographic key generation (port 50055 gRPC)
# =============================================================================

resource "kubernetes_deployment" "plugin_std_crypto" {
  metadata {
    name      = "plugin-std-crypto"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-crypto" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "plugin-std-crypto" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-crypto" })
      }

      spec {
        container {
          name              = "plugin-std-crypto"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_crypto:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_crypto"]
          args    = ["--bind", "tcp://0.0.0.0:50055"]

          port {
            container_port = 50055
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

resource "kubernetes_service" "plugin_std_crypto" {
  metadata {
    name      = "plugin-std-crypto"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "plugin-std-crypto" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "plugin-std-crypto" }

    port {
      port        = 50055
      target_port = 50055
      protocol    = "TCP"
    }
  }
}

# =============================================================================
# plugin-std-container — Container orchestrator (ports 50053 + 50054 gRPC)
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

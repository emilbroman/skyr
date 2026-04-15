# =============================================================================
# Web — Static frontend (port 80)
# =============================================================================

resource "kubernetes_deployment" "web" {
  metadata {
    name      = "web"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "web" })
  }

  spec {
    replicas = 2

    selector {
      match_labels = { "app.kubernetes.io/name" = "web" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "web" })
      }

      spec {
        container {
          name              = "web"
          image             = "ghcr.io/emilbroman/skyr-web:latest"
          image_pull_policy = var.image_pull_policy

          env {
            name  = "API_UPSTREAM"
            value = "api.${local.namespace}.svc.cluster.local:8080"
          }

          port {
            container_port = 80
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

resource "kubernetes_service" "web" {
  metadata {
    name      = "web"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "web" })
  }

  spec {
    type     = var.web_service_type
    selector = { "app.kubernetes.io/name" = "web" }

    port {
      port        = 80
      target_port = 80
      protocol    = "TCP"
    }
  }
}

# =============================================================================
# API — GraphQL endpoint (port 8080)
# =============================================================================

resource "kubernetes_deployment" "api" {
  metadata {
    name      = "api"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "api" })
  }

  spec {
    replicas = 3

    selector {
      match_labels = { "app.kubernetes.io/name" = "api" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "api" })
      }

      spec {
        container {
          name              = "api"
          image             = "ghcr.io/emilbroman/skyr-api:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/api"]
          args = [
            "--host", "0.0.0.0",
            "--port", "8080",
            "--cdb-hostname", local.scylladb_hostname,
            "--rdb-hostname", local.scylladb_hostname,
            "--udb-hostname", local.redis_hostname,
            "--ldb-hostname", local.redpanda_hostname,
            "--adb-endpoint-url", local.minio_endpoint,
            "--adb-bucket", var.minio_bucket,
            "--adb-access-key-id", "$(MINIO_ACCESS_KEY)",
            "--adb-secret-access-key", "$(MINIO_SECRET_KEY)",
            "--adb-region", var.minio_region,
            "--challenge-salt", "$(CHALLENGE_SALT)",
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

          env {
            name = "CHALLENGE_SALT"
            value_from {
              secret_key_ref {
                name = kubernetes_secret.skyr.metadata[0].name
                key  = "challenge-salt"
              }
            }
          }

          port {
            container_port = 8080
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

resource "kubernetes_service" "api" {
  metadata {
    name      = "api"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "api" })
  }

  spec {
    type     = var.api_service_type
    selector = { "app.kubernetes.io/name" = "api" }

    port {
      port        = 8080
      target_port = 8080
      protocol    = "TCP"
    }
  }
}

# =============================================================================
# SCS — Git-over-SSH server (port 2222)
# =============================================================================

resource "kubernetes_deployment" "scs" {
  metadata {
    name      = "scs"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "scs" })
  }

  spec {
    replicas = 3

    selector {
      match_labels = { "app.kubernetes.io/name" = "scs" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "scs" })
      }

      spec {
        volume {
          name = "host-key"
          secret {
            secret_name = kubernetes_secret.skyr.metadata[0].name
            items {
              key  = "host.pem"
              path = "host.pem"
            }
          }
        }

        container {
          name              = "scs"
          image             = "ghcr.io/emilbroman/skyr-scs:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/scs"]
          args = [
            "daemon",
            "--address", "0.0.0.0:2222",
            "--key", "/secrets/host.pem",
            "--cdb-hostname", local.scylladb_hostname,
            "--udb-hostname", local.redis_hostname,
            "--rdb-hostname", local.scylladb_hostname,
            "--node-registry-hostname", local.redis_hostname,
          ]

          volume_mount {
            name       = "host-key"
            mount_path = "/secrets"
            read_only  = true
          }

          port {
            container_port = 2222
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

resource "kubernetes_service" "scs" {
  metadata {
    name      = "scs"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "scs" })
  }

  spec {
    type     = var.scs_service_type
    selector = { "app.kubernetes.io/name" = "scs" }

    port {
      port        = 2222
      target_port = 2222
      protocol    = "TCP"
    }
  }
}

# =============================================================================
# DE — Deployment Engine (no port, internal daemon)
# =============================================================================

resource "kubernetes_deployment" "de" {
  count = var.de_worker_count

  metadata {
    name      = "de-${count.index}"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "de", "skyr/worker-index" = tostring(count.index) })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "de", "skyr/worker-index" = tostring(count.index) }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "de", "skyr/worker-index" = tostring(count.index) })
      }

      spec {
        container {
          name              = "de"
          image             = "ghcr.io/emilbroman/skyr-de:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/de"]
          args = [
            "daemon",
            "--cdb-hostname", local.scylladb_hostname,
            "--rdb-hostname", local.scylladb_hostname,
            "--rtq-hostname", local.rabbitmq_hostname,
            "--ldb-hostname", local.redpanda_hostname,
            "--worker-index", tostring(count.index),
            "--worker-count", tostring(var.de_worker_count),
          ]
        }
      }
    }
  }

  lifecycle {
    ignore_changes = [spec[0].template[0].spec[0].container[0].image]
  }
}

# =============================================================================
# RTE — Resource Transition Engine (multiple workers, no port)
# =============================================================================

resource "kubernetes_deployment" "rte" {
  count = var.rte_worker_count

  metadata {
    name      = "rte-${count.index}"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "rte", "skyr/worker-index" = tostring(count.index) })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "rte", "skyr/worker-index" = tostring(count.index) }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "rte", "skyr/worker-index" = tostring(count.index) })
      }

      spec {
        volume {
          name = "plugin-sockets"
          empty_dir {}
        }

        # --- RTE main container ---
        container {
          name              = "rte"
          image             = "ghcr.io/emilbroman/skyr-rte:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/rte"]
          args = [
            "daemon",
            "--rdb-hostname", local.scylladb_hostname,
            "--rtq-hostname", local.rabbitmq_hostname,
            "--ldb-hostname", local.redpanda_hostname,
            "--plugin", "Std/Random@unix://_/var/run/plugins/random.sock",
            "--plugin", "Std/Artifact@unix://_/var/run/plugins/artifact.sock",
            "--plugin", "Std/Crypto@unix://_/var/run/plugins/crypto.sock",
            "--plugin", "Std/DNS@tcp://plugin-std-dns.${local.namespace}.svc.cluster.local:50057",
            "--plugin", "Std/Time@unix://_/var/run/plugins/time.sock",
            "--plugin", "Std/Container@tcp://plugin-std-container.${local.namespace}.svc.cluster.local:50054",
            "--worker-index", tostring(count.index),
            "--worker-count", tostring(var.rte_worker_count),
            "--local-workers", tostring(var.rte_local_workers),
          ]

          volume_mount {
            name       = "plugin-sockets"
            mount_path = "/var/run/plugins"
          }
        }

        # --- Sidecar: plugin-std-random ---
        container {
          name              = "plugin-std-random"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_random:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_random"]
          args    = ["--bind", "unix://_/var/run/plugins/random.sock"]

          volume_mount {
            name       = "plugin-sockets"
            mount_path = "/var/run/plugins"
          }
        }

        # --- Sidecar: plugin-std-artifact ---
        container {
          name              = "plugin-std-artifact"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_artifact:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_artifact"]
          args = [
            "--bind", "unix://_/var/run/plugins/artifact.sock",
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

          volume_mount {
            name       = "plugin-sockets"
            mount_path = "/var/run/plugins"
          }
        }

        # --- Sidecar: plugin-std-crypto ---
        container {
          name              = "plugin-std-crypto"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_crypto:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_crypto"]
          args    = ["--bind", "unix://_/var/run/plugins/crypto.sock"]

          volume_mount {
            name       = "plugin-sockets"
            mount_path = "/var/run/plugins"
          }
        }

        # --- Sidecar: plugin-std-time ---
        container {
          name              = "plugin-std-time"
          image             = "ghcr.io/emilbroman/skyr-plugin_std_time:latest"
          image_pull_policy = var.image_pull_policy

          command = ["/plugin_std_time"]
          args    = ["--bind", "unix://_/var/run/plugins/time.sock"]

          volume_mount {
            name       = "plugin-sockets"
            mount_path = "/var/run/plugins"
          }
        }
      }
    }
  }

  lifecycle {
    ignore_changes = [spec[0].template[0].spec[0].container[0].image]
  }
}

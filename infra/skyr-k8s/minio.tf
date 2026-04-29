resource "kubernetes_deployment_v1" "minio" {
  count = local.deploy_minio ? 1 : 0

  metadata {
    name      = "minio"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "minio" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "minio" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "minio" })
      }

      spec {
        container {
          name  = "minio"
          image = "minio/minio:RELEASE.2025-09-07T16-13-09Z"

          args = ["server", "/data", "--console-address", ":9001"]

          env {
            name = "MINIO_ROOT_USER"
            value_from {
              secret_key_ref {
                name = kubernetes_secret_v1.skyr.metadata[0].name
                key  = "minio-access-key-id"
              }
            }
          }

          env {
            name = "MINIO_ROOT_PASSWORD"
            value_from {
              secret_key_ref {
                name = kubernetes_secret_v1.skyr.metadata[0].name
                key  = "minio-secret-key"
              }
            }
          }

          dynamic "env" {
            for_each = var.minio_external_url != null ? [var.minio_external_url] : []
            content {
              name  = "MINIO_SERVER_URL"
              value = env.value
            }
          }

          port {
            name           = "s3"
            container_port = 9000
            protocol       = "TCP"
          }

          port {
            name           = "console"
            container_port = 9001
            protocol       = "TCP"
          }
        }
      }
    }
  }
}

resource "kubernetes_service_v1" "minio" {
  count = local.deploy_minio ? 1 : 0

  metadata {
    name      = "minio"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "minio" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "minio" }

    port {
      name        = "s3"
      port        = 9000
      target_port = 9000
      protocol    = "TCP"
    }

    port {
      name        = "console"
      port        = 9001
      target_port = 9001
      protocol    = "TCP"
    }
  }
}

resource "kubernetes_deployment_v1" "redis" {
  count = local.deploy_redis ? 1 : 0

  metadata {
    name      = "redis"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "redis" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "redis" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "redis" })
      }

      spec {
        container {
          name  = "redis"
          image = "redis:8-alpine"

          port {
            container_port = 6379
            protocol       = "TCP"
          }
        }
      }
    }
  }
}

resource "kubernetes_service_v1" "redis" {
  count = local.deploy_redis ? 1 : 0

  metadata {
    name      = "redis"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "redis" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "redis" }

    port {
      port        = 6379
      target_port = 6379
      protocol    = "TCP"
    }
  }
}

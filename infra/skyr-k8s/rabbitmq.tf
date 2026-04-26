resource "kubernetes_deployment_v1" "rabbitmq" {
  count = local.deploy_rabbitmq ? 1 : 0

  metadata {
    name      = "rabbitmq"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "rabbitmq" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "rabbitmq" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "rabbitmq" })
      }

      spec {
        container {
          name  = "rabbitmq"
          image = "rabbitmq:4-management-alpine"

          port {
            name           = "amqp"
            container_port = 5672
            protocol       = "TCP"
          }

          port {
            name           = "management"
            container_port = 15672
            protocol       = "TCP"
          }
        }
      }
    }
  }
}

resource "kubernetes_service_v1" "rabbitmq" {
  count = local.deploy_rabbitmq ? 1 : 0

  metadata {
    name      = "rabbitmq"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "rabbitmq" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "rabbitmq" }

    port {
      name        = "amqp"
      port        = 5672
      target_port = 5672
      protocol    = "TCP"
    }

    port {
      name        = "management"
      port        = 15672
      target_port = 15672
      protocol    = "TCP"
    }
  }
}

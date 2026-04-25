resource "kubernetes_deployment" "mailhog" {
  count = local.deploy_mailhog ? 1 : 0

  metadata {
    name      = "mailhog"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "mailhog" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "mailhog" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "mailhog" })
      }

      spec {
        container {
          name  = "mailhog"
          image = "mailhog/mailhog:latest"

          port {
            name           = "smtp"
            container_port = 1025
            protocol       = "TCP"
          }

          port {
            name           = "http"
            container_port = 8025
            protocol       = "TCP"
          }
        }
      }
    }
  }
}

resource "kubernetes_service" "mailhog" {
  count = local.deploy_mailhog ? 1 : 0

  metadata {
    name      = "mailhog"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "mailhog" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "mailhog" }

    port {
      name        = "smtp"
      port        = 1025
      target_port = 1025
      protocol    = "TCP"
    }

    port {
      name        = "http"
      port        = 8025
      target_port = 8025
      protocol    = "TCP"
    }
  }
}

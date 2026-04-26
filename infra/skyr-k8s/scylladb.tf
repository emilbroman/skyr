resource "kubernetes_deployment_v1" "scylladb" {
  count = local.deploy_scylladb ? 1 : 0

  metadata {
    name      = "scylladb"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "scylladb" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "scylladb" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "scylladb" })
      }

      spec {
        container {
          name  = "scylladb"
          image = "scylladb/scylla"

          port {
            container_port = 9042
            protocol       = "TCP"
          }
        }
      }
    }
  }
}

resource "kubernetes_service_v1" "scylladb" {
  count = local.deploy_scylladb ? 1 : 0

  metadata {
    name      = "scylladb"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "scylladb" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "scylladb" }

    port {
      port        = 9042
      target_port = 9042
      protocol    = "TCP"
    }
  }
}

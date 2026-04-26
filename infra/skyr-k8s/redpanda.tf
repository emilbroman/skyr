resource "kubernetes_deployment_v1" "redpanda" {
  count = local.deploy_redpanda ? 1 : 0

  metadata {
    name      = "redpanda"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "redpanda" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "redpanda" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "redpanda" })
      }

      spec {
        container {
          name  = "redpanda"
          image = "redpandadata/redpanda:v25.3.7"

          args = [
            "redpanda", "start",
            "--kafka-addr", var.redpanda_advertise_host != null ? "internal://0.0.0.0:9092,external://0.0.0.0:19092" : "internal://0.0.0.0:9092",
            "--advertise-kafka-addr", var.redpanda_advertise_host != null ? "internal://redpanda.${local.namespace}.svc.cluster.local:9092,external://${var.redpanda_advertise_host}:${kubernetes_service_v1.redpanda[0].spec[0].port[1].node_port}" : "internal://redpanda.${local.namespace}.svc.cluster.local:9092",
            "--overprovisioned",
            "--smp", "1",
            "--memory", "1G",
            "--reserve-memory", "0M",
            "--check=false",
          ]

          port {
            name           = "kafka"
            container_port = 9092
            protocol       = "TCP"
          }

          dynamic "port" {
            for_each = var.redpanda_advertise_host != null ? [1] : []
            content {
              name           = "kafka-external"
              container_port = 19092
              protocol       = "TCP"
            }
          }
        }
      }
    }
  }
}

resource "kubernetes_service_v1" "redpanda" {
  count = local.deploy_redpanda ? 1 : 0

  metadata {
    name      = "redpanda"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "redpanda" })
  }

  spec {
    type     = var.ldb_service_type
    selector = { "app.kubernetes.io/name" = "redpanda" }

    port {
      name        = "kafka"
      port        = 9092
      target_port = 9092
      protocol    = "TCP"
    }

    dynamic "port" {
      for_each = var.redpanda_advertise_host != null ? [1] : []
      content {
        name        = "kafka-external"
        port        = 19092
        target_port = 19092
        protocol    = "TCP"
      }
    }
  }
}

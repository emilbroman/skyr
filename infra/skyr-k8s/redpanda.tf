resource "kubernetes_deployment" "redpanda" {
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
            "--kafka-addr", "internal://0.0.0.0:9092",
            "--advertise-kafka-addr", "internal://redpanda.${local.namespace}.svc.cluster.local:9092",
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
        }
      }
    }
  }
}

resource "kubernetes_service" "redpanda" {
  count = local.deploy_redpanda ? 1 : 0

  metadata {
    name      = "redpanda"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "redpanda" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "redpanda" }

    port {
      name        = "kafka"
      port        = 9092
      target_port = 9092
      protocol    = "TCP"
    }
  }
}

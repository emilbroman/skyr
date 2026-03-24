resource "kubernetes_deployment" "oci_registry" {
  count = local.deploy_oci_registry ? 1 : 0

  metadata {
    name      = "oci-registry"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "oci-registry" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "oci-registry" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "oci-registry" })
      }

      spec {
        container {
          name  = "oci-registry"
          image = "registry:2"

          port {
            container_port = 5000
            protocol       = "TCP"
          }
        }
      }
    }
  }
}

resource "kubernetes_service" "oci_registry" {
  count = local.deploy_oci_registry ? 1 : 0

  metadata {
    name      = "oci-registry"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "oci-registry" })
  }

  spec {
    type     = var.oci_registry_service_type
    selector = { "app.kubernetes.io/name" = "oci-registry" }

    port {
      port        = 5000
      target_port = 5000
      protocol    = "TCP"
    }
  }
}

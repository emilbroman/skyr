# In-cluster SMTP relay used when no upstream relay (`var.ne_smtp`) is
# configured. Runs Postfix via the `boky/postfix` image, which performs
# direct MX delivery to recipient mail servers. Messages from cluster
# IPs are likely to be rejected by recipient anti-spam systems (no PTR,
# no SPF/DMARC), so this is intended as a working default — production
# deployments should set `var.ne_smtp` to relay through a managed
# service like SES, Mailgun, or Postmark.

resource "kubernetes_deployment_v1" "smtp_relay" {
  count = local.deploy_smtp_relay ? 1 : 0

  metadata {
    name      = "smtp-relay"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "smtp-relay" })
  }

  spec {
    replicas = 1

    selector {
      match_labels = { "app.kubernetes.io/name" = "smtp-relay" }
    }

    template {
      metadata {
        labels = merge(local.labels, { "app.kubernetes.io/name" = "smtp-relay" })
      }

      spec {
        container {
          name  = "postfix"
          image = "boky/postfix:latest"

          port {
            name           = "smtp"
            container_port = 25
            protocol       = "TCP"
          }

          env {
            name  = "HOSTNAME"
            value = "smtp-relay.${local.namespace}.svc.cluster.local"
          }

          env {
            name  = "ALLOWED_SENDER_DOMAINS"
            value = local.ne_smtp_sender_domain
          }

          env {
            name  = "MYNETWORKS"
            value = "127.0.0.0/8 ${var.cluster_cidr}"
          }
        }
      }
    }
  }
}

resource "kubernetes_service_v1" "smtp_relay" {
  count = local.deploy_smtp_relay ? 1 : 0

  metadata {
    name      = "smtp-relay"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "smtp-relay" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "smtp-relay" }

    port {
      name        = "smtp"
      port        = 25
      target_port = 25
      protocol    = "TCP"
    }
  }
}

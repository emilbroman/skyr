# Peer-service DNS aliases.
#
# Skyr binaries resolve peer services by formatting their
# `--service-address-template` with the relevant region. In this single-region
# stack the template is `{service}.<namespace>.svc.cluster.local`, so e.g.
# `cdb.<namespace>.svc.cluster.local` must resolve to ScyllaDB. The aliases
# below provide that mapping: a ClusterIP Service with the peer-service name
# selecting the same pods as the backing Deployment when it's deployed
# internally, or an ExternalName Service CNAMEing to the operator-supplied
# hostname when it isn't.
#
# `node-registry` is the SCOC node registry (Redis); SCS edges resolve it
# per-region. `ias` is its own Deployment defined in services.tf.

# --- ScyllaDB-backed: cdb / gddb / rdb / sdb ---------------------------------

locals {
  scylla_alias_services = ["cdb", "gddb", "rdb", "sdb"]
  redis_alias_services  = ["udb", "node-registry"]
  rabbit_alias_services = ["rtq", "rq", "nq"]
}

resource "kubernetes_service_v1" "scylla_alias_internal" {
  for_each = local.deploy_scylladb ? toset(local.scylla_alias_services) : toset([])

  metadata {
    name      = each.value
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = each.value, "skyr/peer-service-alias" = "scylladb" })
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

resource "kubernetes_service_v1" "scylla_alias_external" {
  for_each = local.deploy_scylladb ? toset([]) : toset(local.scylla_alias_services)

  metadata {
    name      = each.value
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = each.value, "skyr/peer-service-alias" = "external" })
  }

  spec {
    type          = "ExternalName"
    external_name = local.scylladb_hostname
  }
}

# --- Redis-backed: udb / node-registry ---------------------------------------

resource "kubernetes_service_v1" "redis_alias_internal" {
  for_each = local.deploy_redis ? toset(local.redis_alias_services) : toset([])

  metadata {
    name      = each.value
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = each.value, "skyr/peer-service-alias" = "redis" })
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

resource "kubernetes_service_v1" "redis_alias_external" {
  for_each = local.deploy_redis ? toset([]) : toset(local.redis_alias_services)

  metadata {
    name      = each.value
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = each.value, "skyr/peer-service-alias" = "external" })
  }

  spec {
    type          = "ExternalName"
    external_name = local.redis_hostname
  }
}

# --- RabbitMQ-backed: rtq / rq / nq ------------------------------------------

resource "kubernetes_service_v1" "rabbit_alias_internal" {
  for_each = local.deploy_rabbitmq ? toset(local.rabbit_alias_services) : toset([])

  metadata {
    name      = each.value
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = each.value, "skyr/peer-service-alias" = "rabbitmq" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "rabbitmq" }

    port {
      port        = 5672
      target_port = 5672
      protocol    = "TCP"
    }
  }
}

resource "kubernetes_service_v1" "rabbit_alias_external" {
  for_each = local.deploy_rabbitmq ? toset([]) : toset(local.rabbit_alias_services)

  metadata {
    name      = each.value
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = each.value, "skyr/peer-service-alias" = "external" })
  }

  spec {
    type          = "ExternalName"
    external_name = local.rabbitmq_hostname
  }
}

# --- Redpanda-backed: ldb ----------------------------------------------------

resource "kubernetes_service_v1" "ldb_alias_internal" {
  count = local.deploy_redpanda ? 1 : 0

  metadata {
    name      = "ldb"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "ldb", "skyr/peer-service-alias" = "redpanda" })
  }

  spec {
    selector = { "app.kubernetes.io/name" = "redpanda" }

    port {
      port        = 9092
      target_port = 9092
      protocol    = "TCP"
    }
  }
}

resource "kubernetes_service_v1" "ldb_alias_external" {
  count = local.deploy_redpanda ? 0 : 1

  metadata {
    name      = "ldb"
    namespace = local.namespace
    labels    = merge(local.labels, { "app.kubernetes.io/name" = "ldb", "skyr/peer-service-alias" = "external" })
  }

  spec {
    type          = "ExternalName"
    external_name = local.redpanda_hostname
  }
}

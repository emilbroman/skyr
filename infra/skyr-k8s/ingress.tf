# =============================================================================
# Traefik IngressRoutes — websecure entrypoint
# =============================================================================
# Created only when the corresponding hostname variable is set.

resource "kubernetes_manifest" "web_ingressroute" {
  count = var.web_hostname != null ? 1 : 0

  manifest = {
    apiVersion = "traefik.io/v1alpha1"
    kind       = "IngressRoute"
    metadata = {
      name      = "web"
      namespace = local.namespace
      labels    = merge(local.labels, { "app.kubernetes.io/name" = "web" })
    }
    spec = {
      entryPoints = ["websecure"]
      routes = [{
        kind  = "Rule"
        match = "HostSNI(`${var.web_hostname}`)"
        services = [{
          name = kubernetes_service.web.metadata[0].name
          port = 80
        }]
      }]
      tls = {}
    }
  }
}

resource "kubernetes_manifest" "scs_ingressroutetcp" {
  count = var.scs_hostname != null ? 1 : 0

  manifest = {
    apiVersion = "traefik.io/v1alpha1"
    kind       = "IngressRouteTCP"
    metadata = {
      name      = "scs"
      namespace = local.namespace
      labels    = merge(local.labels, { "app.kubernetes.io/name" = "scs" })
    }
    spec = {
      entryPoints = ["websecure"]
      routes = [{
        match = "HostSNI(`${var.scs_hostname}`)"
        services = [{
          name = kubernetes_service.scs.metadata[0].name
          port = 2222
        }]
      }]
      tls = {
        passthrough = true
      }
    }
  }
}

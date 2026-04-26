# --- SSH Host Key ---

resource "tls_private_key" "scs_host_key" {
  count     = var.scs_host_key == null ? 1 : 0
  algorithm = "ED25519"
}

locals {
  # The tls_private_key resource wraps base64 at 64 chars (PEM standard),
  # but the OpenSSH private key format uses 70-char lines. The ssh-key
  # crate (used by russh in SCS) is strict about this, so we re-wrap.
  _scs_key_b64 = var.scs_host_key == null ? replace(
    replace(
      replace(one(tls_private_key.scs_host_key[*].private_key_openssh), "-----BEGIN OPENSSH PRIVATE KEY-----\n", ""),
      "\n-----END OPENSSH PRIVATE KEY-----\n", ""
    ),
    "\n", ""
  ) : ""

  scs_host_key_pem = coalesce(
    var.scs_host_key,
    join("\n", concat(
      ["-----BEGIN OPENSSH PRIVATE KEY-----"],
      [for i in range(ceil(length(local._scs_key_b64) / 70)) :
      substr(local._scs_key_b64, i * 70, min(70, length(local._scs_key_b64) - i * 70))],
      ["-----END OPENSSH PRIVATE KEY-----", ""]
    ))
  )
}

# --- Challenge Salt ---

resource "random_password" "challenge_salt" {
  count   = var.api_challenge_salt == null ? 1 : 0
  length  = 32
  special = false
}

locals {
  challenge_salt = coalesce(var.api_challenge_salt, one(random_password.challenge_salt[*].result))
}

# --- MinIO Credentials (for internal deployment) ---

resource "random_password" "minio_access_key" {
  count   = var.minio_access_key_id == null ? 1 : 0
  length  = 20
  special = false
}

resource "random_password" "minio_secret_key" {
  count   = var.minio_secret_access_key == null ? 1 : 0
  length  = 40
  special = false
}

# --- Kubernetes Secret ---

resource "kubernetes_secret_v1" "skyr" {
  metadata {
    name      = "skyr"
    namespace = local.namespace
    labels    = local.labels
  }

  data = {
    "host.pem"            = local.scs_host_key_pem
    "challenge-salt"      = local.challenge_salt
    "minio-access-key-id" = local.minio_access_key
    "minio-secret-key"    = local.minio_secret_key
  }
}

# --- NE SMTP Credentials ---
#
# Always created so the NE Deployment can mount the secret unconditionally.
# When `var.ne_smtp` is null the values resolve to empty strings — the
# in-cluster Postfix relay does not require auth from cluster pods, and
# the NE binary skips the SASL handshake when both fields are empty.

resource "kubernetes_secret_v1" "ne_smtp" {
  metadata {
    name      = "ne-smtp"
    namespace = local.namespace
    labels    = local.labels
  }

  data = {
    username = local.ne_smtp_username
    password = local.ne_smtp_password
  }
}

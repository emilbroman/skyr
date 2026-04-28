# Std/Crypto Plugin

An [RTP](../rtp/) plugin implementing the cryptographic key, CSR, and certificate resource types from `Std/Crypto`.

## Role in the Architecture

This is one of Skyr's standard library plugins, invoked by the [RTE](../rte/) when deployments use `Std/Crypto` resources. Pure hash and HMAC functions in `Std/Crypto` (e.g. `sha256`, `hmacSha256`) are evaluated directly in [SCLC](../sclc/) extern bindings and do not round-trip through this plugin — the plugin only handles resources that need to persist across deployments.

All key and certificate material is generated in-process using RustCrypto crates (`ed25519-dalek`, `p256`/`p384`/`p521`, `rsa`, `x509-cert`) and emitted as PEM. The plugin holds no persistent state of its own; the RTE/RDB layer keeps the generated outputs.

## Resources

### `Std/Crypto.ED25519PrivateKey`

Generates an Ed25519 key pair.

| | Fields |
|---|--------|
| **Inputs** | (none — `name` is the resource identifier) |
| **Outputs** | `pem` (PKCS#8 PEM private key), `publicKeyPem` (SPKI PEM public key) |

### `Std/Crypto.ECDSAPrivateKey`

Generates an ECDSA key pair on a named curve.

| | Fields |
|---|--------|
| **Inputs** | `curve` (`"P-256"`, `"P-384"`, or `"P-521"`; defaults to `"P-256"` in the SCL binding) |
| **Outputs** | `pem` (PKCS#8 PEM private key), `publicKeyPem` (SPKI PEM public key) |

### `Std/Crypto.RSAPrivateKey`

Generates an RSA key pair. Generation runs on a `spawn_blocking` task to avoid stalling the async runtime.

| | Fields |
|---|--------|
| **Inputs** | `size` (integer bits; minimum 2048, maximum 16384, defaults to 2048 in the SCL binding) |
| **Outputs** | `pem` (PKCS#8 PEM private key), `publicKeyPem` (SPKI PEM public key) |

### `Std/Crypto.CertificationRequest`

Builds and self-signs a PKCS#10 CSR using an existing private key. The resource is identified by a hash of its inputs (computed in the SCLC extern binding) rather than an explicit name, so any input change produces a fresh CSR.

| | Fields |
|---|--------|
| **Inputs** | `privateKeyPem`, `subject` (record with `commonName` and optional `organization`, `organizationalUnit`, `country`, `state`, `locality`), optional `subjectAlternativeNames` (auto-detected as IP, email, or DNS), optional `keyUsage` and `extendedKeyUsage` (lists of named flags) |
| **Outputs** | `pem` (CSR in PEM) |

The plugin sniffs the private key type by attempting PKCS#8 PEM decoding for Ed25519, ECDSA P-256, P-384, P-521, and RSA in turn. P-521 is detected and rejected with a clear error because the `p521` crate does not yet implement the traits required by the `x509-cert` builder.

### `Std/Crypto.CertificateSignature`

Signs an X.509 certificate from a CSR. Supports both CA-signed and self-signed certificates. Like `CertificationRequest`, the resource ID is a hash of the inputs.

| | Fields |
|---|--------|
| **Inputs** | `csrPem`, `privateKeyPem` (signing key), optional `caCertPem` (omit for self-signed), `validity` (record with `before: Time.Instant` and optional `after: Time.Instant`) |
| **Outputs** | `pem` (signed X.509 certificate in PEM) |

The signing private key must match the CSR's public key when self-signing, or the CA certificate's public key when CA-signing — this is verified before issuance. Serial numbers are 20 random bytes with the high bit cleared. When `validity.after` is omitted, `notBefore` defaults to the current system time. P-521 signing is rejected with a clear error.

## Lifecycle

`create_resource` and `update_resource` both delegate to a single dispatch routine that regenerates the requested artifact from inputs. Key resources therefore generate fresh material on update (the SCL bindings deliberately keep `name`-only inputs so that `update` is rare). CSR and certificate resources are content-addressed by input hash, so updates only happen when something meaningful has changed.

There is no `delete` or `check` implementation — the plugin produces opaque PEM strings and leaves lifecycle bookkeeping to the RTE.

## Running

```sh
cargo run -p plugin_std_crypto -- --bind 0.0.0.0:50055
```

## Related Crates

- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions
- [SCLC](../sclc/) — `Std/Crypto` SCL definitions and extern bindings (including the in-process hash/HMAC functions)

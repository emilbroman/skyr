# Skyr Identity Tokens (`auth_token`)

`auth_token` defines the wire format and crypto primitive for **signed identity
tokens** — the bearer tokens that authenticate users across regions in Skyr.

## Role in the Architecture

A user's home region (the region whose UDB owns their PII) is the only place
that can validate that user's credentials (SSH signature, WebAuthn assertion).
Once it has, it issues a self-validating identity token: any other region can
verify the token using the issuer's published public key, without contacting
the issuer or holding any per-user state.

```
                signs                     verifies via
   home region ─────► identity token ─────► any region
   UDB                                       (API edge)
   (private key)                             (cached public key)
```

The public keys themselves are published into a globally-replicated GDDB table
(`region_keys`); see the GDDB crate for how regions register and look them up.

## Wire Format

```
<base64url(payload)>.<base64url(signature)>
```

Signatures are 64-byte Ed25519. The `payload` is a tight binary encoding:

```
1 byte    format version (currently 1)
1 byte    username length (≤ 255)
N bytes   username (UTF-8)
1 byte    issuer region length (≤ 255)
N bytes   issuer region (UTF-8, must parse as ids::RegionId)
8 bytes   issued_at  (i64, big-endian, unix seconds)
8 bytes   expires_at (i64, big-endian, unix seconds)
16 bytes  nonce (issuer-chosen randomness)
```

A typical token (20-char username, 12-char region) is well under 200 bytes
on the wire — small enough to pass through HTTP headers without thinking.

## API

`auth_token` is a tiny library crate with three entry points:

- [`issue`] signs claims with a private key and produces a token string.
- [`parse`] decodes a token string into an [`UnverifiedToken`], exposing the
  issuer region so the caller can look up the right public key.
- [`UnverifiedToken::verify`] checks the signature and expiry.

Splitting parse and verify lets the caller resolve the issuer region (via
GDDB or a local cache) **after** it knows what region issued the token, but
**before** it trusts any claim. The verifier never sees a Claims struct
whose signature hasn't been checked.

## Why Not Protobuf / JWT / CBOR

A 5-field signed envelope doesn't justify pulling in `prost` + `tonic-build`,
the `jsonwebtoken` crate's algorithm-negotiation surface, or a CBOR codec.
The format is small, fixed, and self-contained; the crate has four
dependencies and no build-time codegen.

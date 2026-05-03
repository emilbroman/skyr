# Skyr User Database (UDB)

UDB is an internal library that wraps a Redis client and exposes a typed API for user management, credential storage, and identity-token signing.

## Role in the Architecture

UDB is wrapped by [IAS](../ias/) — every API-edge auth call (sign-up, sign-in, refresh, credential management, every user/org read) goes through IAS over gRPC, and IAS talks to UDB in-process. The one remaining direct UDB consumer is [NE](../ne/), whose recipient-resolution path predates IAS and is expected to migrate to an IAS RPC. Long-term, UDB is expected to be merged into IAS.

```
API edge ──gRPC──▶ IAS ──▶ UDB (Redis)
                       └─▶ Ed25519 signing
```

## Capabilities

| Operation | Description |
|-----------|-------------|
| Register/fetch users | Create and look up user accounts |
| Set full name | Update optional user display name |
| Issue identity tokens | Sign Ed25519 identity tokens (see [`auth_token`](../auth_token/)) |
| Add/check/remove credentials | Per-user public key + WebAuthn credential management |
| Org membership | Create orgs, list/add/remove members |

## Key Prefixes

UDB uses the following Redis key prefixes:

| Prefix | Purpose |
|--------|---------|
| `u:`  | User hashes |
| `p:`  | Per-user public key fingerprint sets |
| `c:`  | Per-user credential records (public_key, credential_id, sign_count) |
| `o:`  | Organization records |
| `m:`  | Per-org member sets |
| `om:` | Per-user org sets |
| `ns:` | Namespace reservation (org or user) |

## Client Construction

Clients are created via `ClientBuilder` and scoped per-user/per-org: `Client` → `.user(username)` → `UserClient` → `.pubkeys()` for credential operations; `Client` → `.org(name)` → `OrgClient` → `.members()` for membership operations.

A `SigningIdentity` (Ed25519 secret + region label) can be attached to the client builder to enable `issue_identity_token`.

## Related Crates

- [IAS](../ias/) — only caller; serves the gRPC surface that the API edge consumes.
- [auth_token](../auth_token/) — wire format and Ed25519 primitives used by `issue_identity_token`.

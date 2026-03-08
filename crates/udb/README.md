# Skyr User Database (UDB)

UDB is a library that wraps a Redis client and exposes a typed API for user management, authentication, and SSH key storage.

## Role in the Architecture

UDB provides the user identity layer for Skyr. It is used by [SCS](../scs/) for SSH authentication and by [API](../api/) for account management and bearer token validation.

```
SCS → UDB ← API
```

## Capabilities

| Operation | Description |
|-----------|-------------|
| Register/fetch users | Create and look up user accounts |
| Set full name | Update optional user display name |
| Issue/revoke bearer tokens | Short-lived tokens for API authentication |
| Add/check/remove SSH pubkeys | Per-user SSH public key fingerprint management |

## Key Prefixes

UDB uses the following Redis key prefixes:

| Prefix | Purpose |
|--------|---------|
| `u:` | User hashes |
| `p:` | Per-user public key sets |
| `t:` | Bearer tokens |

## Related Crates

- [SCS](../scs/) — validates SSH connections against stored pubkeys
- [API](../api/) — issues tokens on signup, validates tokens on requests

# Standard Library Reference

## Std/DNS

DNS resource management.

### DNS.ARecord

Create a DNS A record.

```scl
import Std/DNS
import Std/Time

DNS.ARecord({
    name: "example.com",
    ttl: Time.minute,
    addresses: ["93.184.216.34"],
})
```

| | Fields |
|---|--------|
| **Inputs** | `name: Str` — fully-qualified domain name |
| | `ttl: Time.Duration` — time to live |
| | `addresses: [Str]` — list of IPv4 addresses |
| **Outputs** | Same as inputs |

## Std/Package

Types describing a repository's cross-repo dependency manifest. Used in `Package.scle` files at the root of a repository — see [Cross-Repo Imports](../cross-repo-imports.md).

### Package.Manifest

```scl
export type Manifest {
    dependencies: #{ Str: Str }
}
```

A manifest declares the foreign repositories this repo depends on. Each `dependencies` entry maps `Org/Repo` to a Git-ref-like specifier:

- A bare branch name, e.g. `"main"`.
- A tag, prefixed with `tag:`, e.g. `"tag:v1.2.0"`.
- A 40-character hex commit hash for a deterministic pin.

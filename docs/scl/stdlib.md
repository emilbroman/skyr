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

## Std/Crypto

Cryptographic operations: key generation, certificate management, and hashing.

### Hashing

All hash functions accept a `Str` and return a lowercase hex-encoded digest string. The input is hashed as its UTF-8 bytes.

#### Crypto.sha1

```
sha1: fn(Str) Str
```

Compute the SHA-1 hash of the input. Returns a 40-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.sha1("hello")  // "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
```

#### Crypto.sha256

```
sha256: fn(Str) Str
```

Compute the SHA-256 hash of the input. Returns a 64-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.sha256("hello")  // "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
```

#### Crypto.sha512

```
sha512: fn(Str) Str
```

Compute the SHA-512 hash of the input. Returns a 128-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.sha512("hello")  // "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043"
```

#### Crypto.md5

```
md5: fn(Str) Str
```

Compute the MD5 hash of the input. Returns a 32-character lowercase hex digest.

**Insecure — do not use for authentication or integrity checking. Provided for legacy compatibility only.**

```scl
import Std/Crypto

let digest = Crypto.md5("hello")  // "5d41402abc4b2a76b9719d911017c592"
```

#### Crypto.hmacSha256

```
hmacSha256: fn(Str, Str) Str
```

Compute the HMAC-SHA-256 of a message using a key. The first argument is the key, the second is the message. Both are interpreted as UTF-8 bytes. Returns a 64-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.hmacSha256("key", "The quick brown fox jumps over the lazy dog")
// "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
```

#### Crypto.hmacSha512

```
hmacSha512: fn(Str, Str) Str
```

Compute the HMAC-SHA-512 of a message using a key. The first argument is the key, the second is the message. Both are interpreted as UTF-8 bytes. Returns a 128-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.hmacSha512("key", "The quick brown fox jumps over the lazy dog")
// "b42af09057bac1e2d41708e48a902e09b5ff7f12ab428a4fe86653c73dd248fb82f948a549f7b791a5b41915ee4d1ec3935357e4e2317250d0372afa2ebeeb3a"
```

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

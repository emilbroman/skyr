# Skyr Identifier Types (IDs)

IDs is a shared crate that defines the standard vocabulary and typed identifiers used across all layers of the Skyr architecture — databases, protocols, APIs, and runtime.

## Role in the Architecture

Every crate that references organizations, repositories, environments, deployments, regions, or resources depends on this crate. It is the single source of truth for the deployment namespace hierarchy, the region-scoped resource ID, and the operator-supplied `ServiceAddressTemplate` for region-scoped peer addressing — and it ensures consistent parsing, validation, and formatting throughout the system.

```
IDs ← CDB, SCS, DE, RTE, API, Plugin
```

## Namespace Hierarchy

Skyr's identity types fall into two groups: a four-level **deployment hierarchy** (Org → Repo → Env → Deployment), and a **region-scoped resource ID** that lives below an environment.

| Type | Role | Validation | Example |
|------|------|------------|---------|
| `OrgId` | Organization | SCL symbol | `MyOrg` |
| `RepoId` | Repository (under an org) | SCL symbol | `MyRepo` |
| `EnvironmentId` | Environment (under a repo) | Git ref (stripped) | `main`, `tag:v1.0` |
| `DeploymentId` | Deployment revision | `ObjId.Nonce` | `a10fb43f....a1b2c3d4e5f60718` |
| `RegionId` | Skyr region (metro) | `[a-z]+` | `stockholm`, `paris` |
| `ResourceId` | Region-scoped resource ID | `Region:Type:Name` | `stockholm:Std/Random.Int:seed` |

`RegionId` is **not** a level of the namespace hierarchy — orgs, repos, and environments are not partitioned by region. Region appears in the path of an individual resource because a resource's physical placement is part of its identity. Org and repo names resolve to a home region via [GDDB](../gddb/) at lookup time.

A `DeploymentId` is the pair `(commit, nonce)`: an `ObjId` (40-char lowercase hex SHA-1 of a git object — here, the commit) and a `DeploymentNonce` (16-char lowercase hex `u64`). The nonce distinguishes multiple deployments of the same commit.

SCL symbol validation requires: non-empty, first character alphabetic or `_`, remaining characters alphanumeric or `_`.

A `RegionId` is a deliberately narrow `[a-z]+` label. The live set of regions is operator data — Skyr software contains no region enum, allowlist, or cloud mapping.

## Qualified Identifiers (QIDs)

Each level also has a qualified form that includes all parent scopes:

| Type | Format | Example |
|------|--------|---------|
| `RepoQid` | `org/repo` | `MyOrg/MyRepo` |
| `EnvironmentQid` | `org/repo::env` | `MyOrg/MyRepo::main` |
| `DeploymentQid` | `org/repo::env@hash.nonce` | `MyOrg/MyRepo::main@a10fb43f....a1b2c3d4e5f60718` |
| `ResourceQid` | `org/repo::env::Region:Type:Name` | `MyOrg/MyRepo::main::stockholm:Std/Random.Int:seed` |

### Separators

- `/` between organization and repository
- `::` between repository QID and environment
- `@` between environment QID and deployment
- `.` between commit hash and nonce (within a deployment ID)
- `:` between region and resource type, and between resource type and resource name (within a resource ID)
- `::` between environment QID and resource ID (within a resource QID)

## Environment IDs and Git Refs

Environment IDs are derived from Git refs with the `refs/heads/` or `refs/tags/` prefix stripped:

- **Branches** use bare names: `main`, `feature/login`
- **Tags** use a `tag:` prefix: `tag:v1.0`, `tag:release/2024`

Use `EnvironmentId::from_git_ref()` and `EnvironmentId::to_git_ref()` to convert between the two representations.

## Namespaces

Some infrastructure (RDB, LDB, ADB) accepts any QID level as its partition key. These use the term "namespace" with plain `String` values — the caller decides which QID level to use. For example:

- **RDB** uses environment QIDs as namespaces (resources are grouped per environment).
- **LDB** uses deployment QIDs as namespaces (logs are grouped per deployment).
- **ADB** uses deployment QIDs as namespaces (artifacts belong to a deployment).

## Service Address Template

Region-scoped peer service addresses (e.g. `rq.stockholm.int.skyr.cloud`) are produced by substituting `{service}` and `{region}` into a template that the operator passes to every service binary as `--service-address-template` (default: `{service}.{region}.int.skyr.cloud`).

- `ServiceAddressTemplate` — a validated template string. Validation rejects unknown placeholders (typos like `{regin}` fail at startup, not at first cross-region call).
- `ServiceAddressTemplate::default_template()` — returns the default `{service}.{region}.int.skyr.cloud`.
- `ServiceAddressTemplate::format(service, &RegionId)` — substitutes the placeholders and returns the resulting hostname (the protocol-specific port is concatenated by the caller).

The template is opaque to the rest of Skyr: no binary contains a list of regions or a hard-coded peer-DNS scheme. Single-region deployments can omit the `{region}` placeholder entirely (`{service}.skyr.svc.cluster.local`).

## Type Features

All ID and QID types implement:

- `FromStr` / `Display` — parse from and format to strings
- `Debug` — debug representation
- `Clone`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash` — standard derives
- `Serialize` / `Deserialize` (serde) — JSON-compatible serialization

Leaf ID types use `new_unchecked()` constructors for data already validated (e.g., from the database).

### Builder Methods

QID types provide builder methods for constructing child QIDs:

- `RepoQid::new(OrgId, RepoId)` — construct a repo QID
- `RepoQid::environment(EnvironmentId)` → `EnvironmentQid`
- `EnvironmentQid::new(RepoQid, EnvironmentId)` — construct an environment QID
- `EnvironmentQid::deployment(DeploymentId)` → `DeploymentQid`
- `EnvironmentQid::resource(ResourceId)` → `ResourceQid`
- `DeploymentQid::new(EnvironmentQid, DeploymentId)` — construct a deployment QID
- `ResourceQid::new(EnvironmentQid, ResourceId)` — construct a resource QID

### Parent Accessors

QID types provide accessors for parent scopes:

- `EnvironmentQid::repo_qid()` → `&RepoQid`
- `DeploymentQid::environment_qid()` → `&EnvironmentQid`
- `DeploymentQid::repo_qid()` → `&RepoQid`
- `ResourceQid::environment_qid()` → `&EnvironmentQid`
- `ResourceQid::resource()` → `&ResourceId`

### Binary Encoding

`ObjId` (the type backing the `commit` field of a `DeploymentId`, and any other git object hash referenced by Skyr) supports binary encoding for compact storage:

- `ObjId::from_bytes(&[u8])` — decode from 20-byte SHA-1 representation
- `ObjId::to_bytes()` → `[u8; 20]` — encode to bytes
- `ObjId::as_bytes()` — borrow the 20 bytes
- `ObjId::from_hex(&[u8])` — decode from 40-byte ASCII lowercase hex
- `ObjId::null()` — the all-zero hash (git's null OID)
- `ObjId::hash_bytes(&[u8])` — SHA-1 of raw content (no git framing)
- `ObjId::from_git_object(kind, &[u8])` — SHA-1 of `<kind> <len>\0<data>` (the git object hash)

`ObjId` round-trips losslessly with `gix_hash::ObjectId` via `From`/`Into`/`AsRef` so the rare git-protocol-facing call sites can convert at the boundary without parsing through hex.

## Related Crates

Every crate in the workspace that works with namespace identifiers depends on this crate. Key consumers:

- [CDB](../cdb/) — stores deployments keyed by repo QID + environment + deployment
- [SCS](../scs/) — converts Git refs to environment IDs on push
- [DE](../de/) — uses environment and deployment QIDs for namespace computation
- [RTE](../rte/) — uses deployment QIDs for log namespaces
- [API](../api/) — parses deployment QIDs from resource owner strings
- [RTQ](../rtq/) — messages reference deployment QIDs as owner identifiers
- [GDDB](../gddb/) — uses `name_hash` (SHA-1 of the lowercased name) to key its case-insensitive name reservations
- Every region-aware service binary (`api`, `scs`, `de`, `rte`, `re`, `ne`) — accepts `--service-address-template` to resolve peer hostnames

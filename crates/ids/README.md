# Skyr Identifier Types (IDs)

IDs is a shared crate that defines the standard vocabulary and typed identifiers used across all layers of the Skyr architecture — databases, protocols, APIs, and runtime.

## Role in the Architecture

Every crate that references organizations, repositories, environments, or deployments depends on this crate. It provides a single source of truth for the four-level namespace hierarchy and ensures consistent parsing, validation, and formatting throughout the system.

```
IDs ← CDB, SCS, DE, RTE, API, Plugin
```

## Namespace Hierarchy

Skyr organizes infrastructure into four levels:

| Level | Type | Validation | Example |
|-------|------|------------|---------|
| **Organization** | `OrgId` | SCL symbol | `MyOrg` |
| **Repository** | `RepoId` | SCL symbol | `MyRepo` |
| **Environment** | `EnvironmentId` | Git ref (stripped) | `main`, `tag:v1.0` |
| **Deployment** | `DeploymentId` | 40-char hex SHA-1 | `a10fb43f...` |
| **Resource** | `ResourceId` | `Type:Name` | `Std/Random.Int:seed` |

SCL symbol validation requires: non-empty, first character alphabetic or `_`, remaining characters alphanumeric or `_`.

## Qualified Identifiers (QIDs)

Each level also has a qualified form that includes all parent scopes:

| Type | Format | Example |
|------|--------|---------|
| `RepoQid` | `org/repo` | `MyOrg/MyRepo` |
| `EnvironmentQid` | `org/repo::env` | `MyOrg/MyRepo::main` |
| `DeploymentQid` | `org/repo::env@deploy` | `MyOrg/MyRepo::main@a10fb43f...` |
| `ResourceQid` | `org/repo::env::Type:Name` | `MyOrg/MyRepo::main::Std/Random.Int:seed` |

### Separators

- `/` between organization and repository
- `::` between repository QID and environment
- `@` between environment QID and deployment
- `:` between resource type and resource name (within a resource ID)
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

`DeploymentId` supports binary encoding for compact storage:

- `DeploymentId::from_bytes(&[u8])` — decode from 20-byte SHA-1 representation
- `DeploymentId::to_bytes()` → `[u8; 20]` — encode to bytes

## Related Crates

Every crate in the workspace that works with namespace identifiers depends on this crate. Key consumers:

- [CDB](../cdb/) — stores deployments keyed by repo QID + environment + deployment
- [SCS](../scs/) — converts Git refs to environment IDs on push
- [DE](../de/) — uses environment and deployment QIDs for namespace computation
- [RTE](../rte/) — uses deployment QIDs for log namespaces
- [API](../api/) — parses deployment QIDs from resource owner strings
- [RTQ](../rtq/) — messages reference deployment QIDs as owner identifiers

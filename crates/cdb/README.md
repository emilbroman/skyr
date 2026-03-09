# Skyr Configuration Database (CDB)

CDB is a library that wraps a Cassandra (ScyllaDB) client and exposes a typed API for interacting with the configuration database.

## Role in the Architecture

CDB is the central persistence layer for Git objects and deployment metadata. It is used by [SCS](../scs/) to store pushed configuration and by [DE](../de/) to read deployment state and configuration files.

```
SCS → CDB ← DE
      CDB ← API
```

## Capabilities

### Git Object Storage

- Store and retrieve raw Git objects (blobs, trees, commits).
- Object-level access for structured reads.

### Deployment Management

- Store and query deployments keyed by repository QID (`org/repo`), environment ID, and deployment ID.
- Update deployment states (Desired, Lingering, Undesired, Down).
- Record and look up supercession relationships between deployments.
- Look up deployments by their full deployment QID (`org/repo::env@deploy`).

### Data Model

CDB uses the [IDs](../ids/) crate for all identifier types:

- **`RepoQid`** (`org/repo`) — identifies a repository; partition key for most tables.
- **`EnvironmentId`** — derived from Git refs (e.g., `main`, `tag:v1.0`); stored as the `environment_id` column.
- **`DeploymentId`** — 40-character hex commit hash; identifies a specific deployment revision.

Client hierarchy: `Client → repo(RepoQid) → RepositoryClient → deployment(EnvironmentId, DeploymentId) → DeploymentClient`.

### Configuration File Access

- `read_file` — read a file from a commit tree by path.
- `read_dir` — list directory entries in a commit tree.

Used by DE to load `Main.scl` and resolve imports during compilation.

## Schema

CDB manages its own keyspace and table creation. When changing the schema, update both table creation statements and prepared statements together.

Note: the codebase consistently uses the spelling `supercede`/`supercession` in schema and API names.

## Related Crates

- [IDs](../ids/) — typed identifiers used throughout CDB
- [SCS](../scs/) — writes Git objects and deployments on push
- [DE](../de/) — reads deployments and config files
- [API](../api/) — reads deployments and objects for the GraphQL API

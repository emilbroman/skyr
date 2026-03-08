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

- Store and query deployments and active deployments.
- Update deployment states (Desired, Lingering, Undesired, Down).
- Record and look up supercession relationships between deployments.

### Configuration File Access

- `read_file` — read a file from a commit tree by path.
- `read_dir` — list directory entries in a commit tree.

Used by DE to load `Main.scl` and resolve imports during compilation.

## Schema

CDB manages its own keyspace and table creation. When changing the schema, update both table creation statements and prepared statements together.

Note: the codebase consistently uses the spelling `supercede`/`supercession` in schema and API names.

## Related Crates

- [SCS](../scs/) — writes Git objects and deployments on push
- [DE](../de/) — reads deployments and config files
- [API](../api/) — reads deployments and objects for the GraphQL API

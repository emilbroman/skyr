# Skyr Resource Database (RDB)

RDB is a library that wraps a Cassandra (ScyllaDB) client and exposes a typed API for interacting with the resource database.

## Role in the Architecture

RDB stores the current state of all resources managed by Skyr. It is written to by the [RTE](../rte/) after processing transitions and read by the [DE](../de/) during reconciliation.

```
RTE → RDB ← DE
```

## Capabilities

### ResourceClient

| Operation | Description |
|-----------|-------------|
| `get` | Retrieve a resource by ID |
| `set_input` | Store the desired inputs and owner for a resource |
| `set_output` | Store the actual outputs from a resource |
| `set_dependencies` | Record resource dependency relationships |
| `delete` | Remove a resource |

### NamespaceClient

| Operation | Description |
|-----------|-------------|
| `list_resources` | List all resources in a namespace |
| `list_resources_by_owner` | List resources owned by a specific deployment |

## Data Model

Each resource has:
- **Inputs** — the desired configuration provided by the deployment
- **Outputs** — the actual state reported by the plugin after creation
- **Dependencies** — list of other resource IDs this resource depends on (stored as JSON)
- **Owner** — the deployment QID (`org/repo::env@deploy`) that owns this resource

Resources are grouped by **namespace**, which is the environment QID (`org/repo::env`). All deployments within the same environment share a resource namespace, enabling seamless resource adoption during rollouts.

## Client Hierarchy

Clients are constructed via a builder and scoped progressively:

`ClientBuilder::build()` → `Client` → `.namespace(env_qid)` → `NamespaceClient` → `.resource(type, id)` → `ResourceClient`

The client automatically creates its keyspace and tables on initialization.

## Related Crates

- [RTE](../rte/) — writes resource state after processing transitions
- [DE](../de/) — reads resource state for reconciliation
- [RTQ](../rtq/) — transition messages reference resources by namespace and ID

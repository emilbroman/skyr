# Skyr API

The API service exposes a GraphQL endpoint for user management, deployment inspection, and artifact access.

## Role in the Architecture

The API is the public-facing HTTP service for Skyr. It provides the GraphQL interface that the [CLI](../cli/) and other clients use for account management and deployment visibility.

```
Client (GraphQL/HTTP) → API → UDB, CDB, ADB, LDB
```

## Endpoints

- **GraphQL** — primary API endpoint
- **GraphiQL** — interactive GraphQL explorer UI

## Operations

| Operation | Type | Description |
|-----------|------|-------------|
| `signup(username, email)` | Mutation | Creates a user account and issues a bearer token |
| `me` | Query | Returns the authenticated user (requires bearer token) |
| Deployment artifacts | Query | Exposes deployment artifact data |

Authentication uses bearer tokens issued by [UDB](../udb/).

## Schema

The GraphQL schema is defined in `schema.graphql`. When the server implementation changes in a way that impacts the schema, regenerate it:

```sh
cargo run -p api -- --write-schema
```

## Related Crates

- [UDB](../udb/) — user accounts and bearer token management
- [CDB](../cdb/) — deployment and Git object data
- [ADB](../adb/) — artifact storage
- [LDB](../ldb/) — deployment logs

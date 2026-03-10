# Skyr API

The API service exposes a GraphQL endpoint for user management, deployment inspection, and artifact access.

## Role in the Architecture

The API is the public-facing HTTP service for Skyr. It provides the GraphQL interface that the [CLI](../cli/) and other clients use for account management and deployment visibility.

```
Client (GraphQL/HTTP) → API → UDB, CDB, RDB, ADB, LDB
```

## Endpoints

- **GraphQL** — primary API endpoint
- **GraphiQL** — interactive GraphQL explorer UI

## Authentication

The API uses SSH challenge-response authentication:

1. Client calls `authChallenge(username)` to get a time-limited challenge string.
2. Client signs the challenge with their SSH private key.
3. Client submits `signup` or `signin` with the username, public key (OpenSSH format), and signature.
4. Server verifies the signature and issues a bearer token.
5. Subsequent requests include the token in the `Authorization: Bearer <token>` header.

Challenges are frame-based (10-second windows) with ±1 frame tolerance for clock skew.

## Operations

### Queries

| Operation | Description |
|-----------|-------------|
| `health` | Service health check |
| `me` | Returns the authenticated user (requires bearer token) |
| `authChallenge(username)` | Returns a challenge string for signing |
| `repositories` | Lists repositories owned by the authenticated user's organization |

### Mutations

| Operation | Description |
|-----------|-------------|
| `signup(username, email, pubkey, signature)` | Creates a user account, stores the public key, and issues a bearer token |
| `signin(username, signature, pubkey)` | Authenticates an existing user and issues a bearer token |
| `createRepository(organization, repository)` | Creates a new repository (organization must match the authenticated user) |

### Subscriptions

| Operation | Description |
|-----------|-------------|
| `deploymentLogs(deploymentId, initialAmount)` | Streams logs for a specific deployment |
| `environmentLogs(environmentQid, initialAmount)` | Streams merged logs from all deployments in an environment |

Log subscriptions use WebSocket transport. `initialAmount` defaults to 1000 and controls how many historical log entries are returned before following new logs.

## GraphQL Types

The schema exposes a nested hierarchy:

- **Repository** — `name`, `environments`
  - **Environment** — `name`, `qid`, `deployments`, `resources`, `lastLogs(amount)`
    - **Deployment** — `id`, `ref`, `commit`, `createdAt`, `state`, `resources`, `artifacts`, `lastLogs(amount)`
      - **Resource** — `type`, `id`, `inputs`, `outputs`, `owner`, `dependencies`
      - **Artifact** — `namespace`, `name`, `mediaType`, `url` (presigned, 15-minute expiry)
- **Log** — `severity` (INFO/WARNING/ERROR), `timestamp`, `message`
- **DeploymentState** — DOWN, UNDESIRED, LINGERING, DESIRED

## Schema

The GraphQL schema is defined in `schema.graphql`. When the server implementation changes in a way that impacts the schema, regenerate it:

```sh
cargo run -p api -- --write-schema
```

## Namespace Usage

The API uses the [IDs](../ids/) crate to work with qualified identifiers:

- Deployment IDs exposed via GraphQL are full deployment QIDs (`org/repo::env@deploy`).
- Resource owner resolution parses deployment QIDs from owner strings to look up the owning deployment.
- Deployment log subscriptions validate that the deployment belongs to the authenticated user's organization.

## Related Crates

- [IDs](../ids/) — typed identifiers for deployment QID parsing
- [UDB](../udb/) — user accounts and bearer token management
- [CDB](../cdb/) — deployment and Git object data
- [RDB](../rdb/) — resource state
- [ADB](../adb/) — artifact storage
- [LDB](../ldb/) — deployment logs

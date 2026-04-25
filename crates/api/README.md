# Skyr API

The API service exposes a GraphQL endpoint for user management, deployment inspection, and artifact access.

## Role in the Architecture

The API is the public-facing HTTP service for Skyr. It provides the GraphQL interface that the [CLI](../cli/) and other clients use for account management and deployment visibility.

```
Client (GraphQL/HTTP) → API → UDB, CDB, RDB, ADB, LDB, SDB
```

The API is a **read-only** consumer of [SDB](../sdb/) — it never writes status or incident records. SDB writes are exclusively the [RE](../re/)'s responsibility.

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
| `Organization.incident(id)` | Single-incident lookup, scoped to the organization. Globally-unique incident IDs are not exposed at the top-level `Query` so that authorization stays visible in the query shape. |
| `Organization.incidents(...)` / `Repository.incidents(...)` / `Environment.incidents(...)` / `Deployment.incidents(...)` / `Resource.incidents(...)` | Scoped incident listings. Common filter args: `entityQid`, `category`, `openOnly`, `since`, `until`, plus `limit` / `offset` pagination. |

### Mutations

| Operation | Description |
|-----------|-------------|
| `signup(username, email, pubkey, signature)` | Creates a user account, stores the public key, and issues a bearer token |
| `signin(username, signature, pubkey)` | Authenticates an existing user and issues a bearer token |
| `createRepository(organization, repository)` | Creates a new repository (organization must match the authenticated user) |

There are no incident mutations: incident lifecycle is RE-driven, and the *closure-is-permanent + recurrence-creates-new-incident* rule makes manual open/close meaningless.

### Subscriptions

| Operation | Description |
|-----------|-------------|
| `deploymentLogs(deploymentId, initialAmount)` | Streams logs for a specific deployment |
| `environmentLogs(environmentQid, initialAmount)` | Streams merged logs from all deployments in an environment |

Log subscriptions use WebSocket transport. `initialAmount` defaults to 1000 and controls how many historical log entries are returned before following new logs.

## GraphQL Types

The schema exposes a nested hierarchy:

- **Repository** — `name`, `environments`, `incidents`
  - **Environment** — `name`, `qid`, `deployments`, `resources`, `lastLogs(amount)`, `incidents`
    - **Deployment** — `id`, `ref`, `commit`, `createdAt`, `state`, `status`, `resources`, `artifacts`, `lastLogs(amount)`, `incidents`
      - **Resource** — `type`, `id`, `inputs`, `outputs`, `owner`, `dependencies`, `status`, `incidents`
      - **Artifact** — `namespace`, `name`, `mediaType`, `url` (presigned, 15-minute expiry)
- **Log** — `severity` (INFO/WARNING/ERROR), `timestamp`, `message`
- **DeploymentState** — DOWN, UNDESIRED, LINGERING, DESIRED
- **StatusSummary** — `health` (HEALTHY/DEGRADED/DOWN), `lastReportAt`, `lastReportSucceeded`, `openIncidentCount`, `worstOpenCategory`, `consecutiveFailureCount`. Read directly from SDB.
- **HealthStatus** — UI-friendly rollup. `HEALTHY` iff no open incidents; `DOWN` iff the worst open category is `CRASH`; `DEGRADED` otherwise.
- **IncidentCategory** — `BAD_CONFIGURATION`, `CANNOT_PROGRESS`, `INCONSISTENT_STATE`, `SYSTEM_ERROR`, `CRASH`. Mirrors the producer-classified severity from `rq::IncidentCategory` / `sdb::Category`.
- **Incident** — `id`, `entityQid`, `category`, `openedAt`, `closedAt`, `lastReportAt`, `reportCount`, `lastErrorMessage`, `triggeringReportSummary`, plus back-edges (`organization`, `repository`, `environment`, `deployment`, `resource`).

`Deployment.status` and `Resource.status` are **self-only** rollups — they reflect only the entity's own incidents. Resource health is reached explicitly via `Deployment.resources -> Resource.status`. This keeps "is the deployment itself failing" and "is one of its resources failing" as distinct, separately-actionable signals.

Historical incidents on a destroyed resource remain reachable via `Environment.incidents(entityQid: ...)` even after `Resource.status` is no longer applicable.

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
- [SDB](../sdb/) — per-entity health summaries and incident records
- [ADB](../adb/) — artifact storage
- [LDB](../ldb/) — deployment logs

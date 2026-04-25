# Skyr Deployment Engine (DE)

DE is a daemon that watches the [CDB](../cdb/) for active deployments and runs a reconciliation loop for each one.

## Role in the Architecture

DE is the heart of Skyr's reconciliation model. It continuously monitors active deployments, compiles their SCL configuration using [SCLC](../sclc/), and drives deployments through their lifecycle states.

```
CDB → DE → SCLC (compile)
           DE → RDB (read current state)
           DE → RTQ (transition requests)
           DE → LDB (deployment logs)
```

## Horizontal Scaling

DE supports horizontal scaling via deployment sharding. Each replica receives a disjoint subset of active deployments based on the commit hash:

```
de daemon --worker-index 0 --worker-count 3
de daemon --worker-index 1 --worker-count 3
de daemon --worker-index 2 --worker-count 3
```

The first 16 hex characters of each deployment's commit hash are interpreted as a big-endian `u64`. The deployment is assigned to a worker via `hash_prefix % worker_count`. With the defaults (`--worker-index 0 --worker-count 1`), a single instance handles all deployments.

## How It Works

1. **Polling**: The daemon polls for active deployments every 20 seconds.
2. **Sharding**: Each instance filters deployments to only those assigned to its worker index.
3. **Workers**: A dedicated worker is spawned for each owned deployment, running a loop every 5 seconds.
4. **State handling**: Each worker handles the deployment based on its current state:

| State | Behavior |
|-------|----------|
| **Desired** | Compiles `Main.scl`, evaluates the resource DAG against current RDB state, and emits transition requests. Once evaluation is complete (no pending effects), marks superseded deployments as Undesired. |
| **Lingering** | Follows the supersession chain to find the active Desired deployment, then marks itself as superseded by it. Includes cycle detection to prevent infinite loops. |
| **Undesired** | Tears down owned resources by enqueuing Destroy messages. Blocks teardown for resources that still have living dependents. Transitions to Down when all owned resources are destroyed. |
| **Down** | Idles. |

## Reconciliation Loop

When a deployment is in the **Desired** state, DE performs a full reconciliation:

1. **Compile**: Runs `sclc::compile()` on `Main.scl` from the deployment's commit. Diagnostics (warnings and errors) are logged to LDB.
2. **Load current state**: Fetches all resources from the RDB namespace (environment QID) and feeds them into the evaluator for comparison.
3. **Evaluate**: Runs the compiled program against the current state, producing effects for any differences.
4. **Emit transitions**: Converts effects into RTQ messages:

| Effect | RTQ Message |
|--------|-------------|
| `CreateResource` | **Create** — new resource to be created by a plugin |
| `UpdateResource` (unowned) | **Restore** — re-apply desired inputs to an existing resource |
| `UpdateResource` (owned by another deployment) | **Adopt** — transfer ownership and optionally update inputs |
| `TouchResource` (owned by another deployment) | **Adopt** — transfer ownership without input changes |

5. **Completeness**: If no effects were emitted, evaluation is **Complete** and superseded deployments can be transitioned to Undesired. If effects were emitted, evaluation is **Partial** and supersession teardown is deferred until the next loop iteration.

## Supersession

When a Desired deployment finishes a complete evaluation, it transitions any Lingering deployments it supersedes to Undesired, triggering their teardown. Lingering deployments follow the supersession chain (via `get_superseding()`) to find the active Desired deployment and record the relationship.

During **Undesired** teardown, DE enqueues Destroy messages for owned resources but blocks destruction of resources that still have living dependents from other deployments. This ensures correct teardown ordering.

## Namespace Usage

DE uses environment QIDs (`org/repo::env`) as the RDB namespace for resource grouping, and deployment QIDs (`org/repo::env@deploy`) as LDB namespaces for log grouping. This ensures resources are shared within an environment while logs remain deployment-specific.

## Status Reporting and Backpressure

The DE is a status-reporting producer and an SDB reader.

- **Producer.** Every loop iteration — including idle "check" iterations and backed-off iterations — emits a single deployment-scoped report onto the [RQ](../rq/). The report carries the deployment QID, wall time, the deployment's current operational state (`DESIRED` / `LINGERING` / `UNDESIRED` / `DOWN`), and a *terminal flag* set on the single last report emitted when the deployment reaches `DOWN`. Failure reports are producer-classified: SCL compile errors as `BadConfiguration`, infrastructure failures (CDB / RDB / RTQ / LDB / SDB unavailability) as `SystemError`. The producer never changes its reporting behavior based on SDB state — it is one-way.
- **Reader.** At the top of every iteration the worker reads two SDB signals:
  - `consecutive_failure_count` from the deployment's status summary, fed into the existing exponential-backoff formula.
  - An open-`Crash` check across the deployment and any of its resources; when set, the iteration is skipped with a success heartbeat (the RE knows the worker is alive; we just are not making progress while the crash is investigated).

These two reads are the only feedback loop from SDB into the DE. The legacy `failures` column on `cdb.deployments` no longer exists — backoff is sourced exclusively from SDB.

## Related Crates

- [IDs](../ids/) — typed identifiers for namespace computation
- [CDB](../cdb/) — source of deployment metadata and configuration files
- [SCLC](../sclc/) — compiles SCL configuration
- [RTQ](../rtq/) — target for transition requests
- [RDB](../rdb/) — resource state for reconciliation
- [RQ](../rq/) — target for status reports
- [SDB](../sdb/) — source of backoff and Crash-eligibility signals
- [LDB](../ldb/) — deployment log output
- [SCS](../scs/) — creates the deployments that DE monitors

See [Deployments](../../docs/deployments.md) for the full deployment lifecycle.

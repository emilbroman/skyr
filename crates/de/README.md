# Skyr Deployment Engine (DE)

DE is a daemon that watches the [CDB](../cdb/) for active deployments and runs a reconciliation loop for each one.

## Role in the Architecture

DE is the heart of Skyr's reconciliation model. It continuously monitors active deployments, compiles their SCL configuration using [SCLC](../sclc/), and drives deployments through their lifecycle states.

```
CDB → DE → SCLC (compile)
           DE → RTQ (transition requests) [not yet implemented]
```

## How It Works

1. **Polling**: The daemon polls for active deployments every 20 seconds.
2. **Workers**: A dedicated worker is spawned for each active deployment, running a loop every 5 seconds.
3. **State handling**: Each worker handles the deployment based on its current state:

| State | Behavior |
|-------|----------|
| **Desired** | Compiles `Main.scl` from the deployment's commit. Marks any superceded deployment as Undesired. |
| **Lingering** | Compiles `Main.scl` and logs. Waiting for the new deployment to take over. |
| **Undesired** | Logs teardown intent and transitions to Down. |
| **Down** | Idles. |

## Current Limitations

DE currently compiles configuration but does not yet emit transition requests to the [RTQ](../rtq/). The planned reconciliation loop — where DE evaluates the compiled resource DAG, compares it against the [RDB](../rdb/), and emits Create/Restore/Adopt/Destroy messages — is not yet implemented.

## Related Crates

- [CDB](../cdb/) — source of deployment metadata and configuration files
- [SCLC](../sclc/) — compiles SCL configuration
- [RTQ](../rtq/) — target for transition requests (planned)
- [RDB](../rdb/) — resource state for reconciliation (planned)
- [SCS](../scs/) — creates the deployments that DE monitors

See [Deployments](../../docs/deployments.md) for the full deployment lifecycle.

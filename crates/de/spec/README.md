# Deployment Engine — TLA+ Specification

This directory contains a TLA+ model of the Deployment Engine's lifecycle state machine. The model captures the core invariants that the implementation must uphold: safe resource ownership, correct supersession ordering, and guaranteed convergence.

## What the model covers

The model focuses on the lifecycle of deployments within a single environment. Everything is implicitly scoped to one environment — there is no cross-environment interaction modeled.

### Deployments

A deployment is identified by a commit hash and a random nonce (the same commit can be deployed multiple times, but each deployment is distinct). A deployment carries a lifecycle label, a bootstrapped flag, and a failure counter:

- **Label**: `DESIRED`, `LINGERING`, `UNDESIRED`, or `DOWN`.
- **Bootstrapped**: whether the deployment's resource DAG has been fully explored to stability at least once.
- **Failures**: consecutive failed iterations (used for exponential backoff). This is observational — it does not alter lifecycle transitions.

### Supersession

Deployments form a singly-linked chain via supersession. When a new deployment arrives, the current one is superseded. The chain only grows (supersessions are never removed). The "current" deployment is the one at the head — the unsuperseded one.

### Resources

Each deployment defines a set of resources with a dependency relation (a DAG). Resources are identified by type and name, and they persist across deployments: if two deployments reference the same resource identity, they refer to the same underlying resource.

A desired deployment adopts or creates resources from the environment's shared pool based on resource identity. Creation respects topological order (a resource's dependencies must be alive first). Destruction respects reverse topological order (a resource is only destroyed when nothing alive depends on it).

### Lifecycle

The full lifecycle of a deployment:

1. **DESIRED** — The deployment is active and reconciling. It iteratively creates, updates, or adopts resources until every resource in its DAG is alive and matching. Once no more transitions are needed, it becomes bootstrapped.

2. **LINGERING** — The deployment has been superseded. It idles until the current deployment (evaluated dynamically — if the current is itself superseded, wait for the new one) is bootstrapped.

3. **UNDESIRED** — The current deployment is bootstrapped, so this deployment tears down resources it owns that are no longer needed. Teardown is the set of resources owned by this deployment minus those in the current deployment's resource set. Multiple undesired deployments may emit overlapping destroy messages for the same resource; this is safe because resource transitions are idempotent.

4. **DOWN** — Terminal. All excess resources have been destroyed. The deployment performs no further work.

## Safety invariants

The model checks three invariants:

- **TypeOK** — All variables stay within their declared domains.
- **CurrentResourcesSafe** — Once the current deployment is bootstrapped, its resources remain alive. Undesired deployments only destroy resources outside the current set.
- **NoResourceContention** — At most one deployment is actively creating/adopting resources at any time (only the unsuperseded desired deployment operates on resources).

## Liveness

Under fair scheduling (every enabled action eventually fires, except deployment creation which is an external event), the model checks that every superseded deployment eventually reaches DOWN.

## Model checking configuration

`MC.tla` and `MC.cfg` configure a small instance for TLC: 3 deployments, 3 resources (`r1`, `r2`, `r3`) with a linear dependency chain (`r3 → r2 → r1`). Deadlock checking is disabled because the system can legitimately quiesce when all deployments are down and no new ones arrive.

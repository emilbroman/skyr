# Deployments

Deployments are the core unit of infrastructure in Skyr. When you push code to a Skyr repository, Skyr creates a deployment and begins rolling out your infrastructure.

## Environments and Deployments

Skyr organizes infrastructure using a four-level hierarchy: **Organization** → **Repository** → **Environment** → **Deployment**.

An **environment** corresponds to a Git branch or tag in your repository. Each environment can have one active deployment at a time. A **deployment** is a specific revision (commit) of an environment.

```bash
git push skyr main
```

This creates a deployment identified by a qualified identifier (QID) that combines all four levels:

```
alice/my_app::main@a10fb43f8a36c9be604dac6e76bd5bb298d3ea2e
│     │       │    └─ deployment (commit hash)
│     │       └─ environment (branch name)
│     └─ repository
└─ organization
```

Separators: `/` (org/repo), `::` (repo::env), `@` (env@deploy).

Tags are also supported as environments using a `tag:` prefix (e.g., `tag:v1.0`).

The deployment reads your `Main.scl` file, evaluates your configuration, and creates the resources you defined.

## The Deployment Lifecycle

Deployments go through a series of states as they roll out and eventually wind down:

### Desired

When you push a new commit, the deployment starts in the **Desired** state. Skyr actively works to make reality match your configuration:

1. Compiles and evaluates your `Main.scl`
2. Creates any new resources your configuration defines
3. Adopts existing resources that match your configuration
4. Updates resources whose inputs have changed

The deployment stays in this state as long as you want it running.

### Lingering

When you push a new commit to the same environment (branch), the old deployment transitions to **Lingering**. It's being replaced, but Skyr keeps it around temporarily while the new deployment rolls out.

During this phase:
- The old deployment stops creating new resources
- It waits for the new deployment to take over its resources
- Resources shared between old and new deployments are adopted by the new one

### Undesired

Once the new deployment has fully rolled out, the old deployment transitions to **Undesired**. Skyr begins tearing down any resources that weren't adopted by the new deployment:

1. Identifies resources owned exclusively by the old deployment
2. Respects dependencies — a resource isn't destroyed until nothing depends on it
3. Destroys resources in the correct order

### Down

After all resources are destroyed, the deployment reaches the **Down** state. It's now complete and no longer active.

## Supersession: How Rollouts Work

When you push a new commit to an environment that already has an active deployment, the new deployment *supersedes* the old one.

```
Before push:
  main → commit A (Desired)

After push:
  main → commit A (Lingering) ──superseded by──→ commit B (Desired)
```

During supersession:

1. **New deployment starts rolling out** — It evaluates your configuration and begins creating resources.

2. **Shared resources are adopted** — If your new configuration includes resources that already exist from the old deployment, Skyr transfers ownership rather than recreating them. If the inputs changed, Skyr updates the resource.

3. **Old deployment waits** — It stays in Lingering until the new deployment finishes its initial rollout.

4. **Old deployment tears down** — Once the new deployment is stable, the old one transitions to Undesired and cleans up orphaned resources.

### Example

Consider this initial configuration:

```scl
import Std/Random

let a = Random.Int({ name: "a", min: 1, max: 10 })
let b = Random.Int({ name: "b", min: 1, max: 10 })
```

You push this and deployment A creates resources `a` and `b`.

Now you update the configuration:

```scl
import Std/Random

let b = Random.Int({ name: "b", min: 1, max: 20 })  // Changed max
let c = Random.Int({ name: "c", min: 1, max: 10 })  // New resource
```

When you push:
1. Deployment B starts (Desired)
2. Resource `b` is adopted by B, and updated because `max` changed
3. Resource `c` is created
4. Deployment A transitions to Undesired
5. Resource `a` is destroyed (no longer in the config)
6. Deployment A transitions to Down

## Deleting Environments

If you delete a branch or tag (or force-push to remove a ref), the environment's deployment transitions directly to Undesired and begins tearing down all its resources.

```bash
git push skyr --delete feature-branch
```

## Resource Namespacing

Resources are grouped by **environment** — all deployments within the same environment share the same resource namespace. This means a resource created by one deployment in `alice/my_app::main` is visible to the next deployment in that same environment, enabling seamless adoption during rollouts.

The resource owner is tracked as a full deployment QID (e.g., `alice/my_app::main@a10fb43f...`), so Skyr always knows exactly which deployment owns each resource.

## Resource Ownership

Every resource in Skyr is owned by exactly one deployment. This ownership model enables clean rollouts:

- **During creation**: The deployment that creates a resource becomes its owner.
- **During adoption**: Ownership transfers from the old deployment to the new one.
- **During teardown**: Only the owning deployment can destroy a resource.

This ensures resources aren't accidentally destroyed during rollouts and that cleanup happens in an orderly fashion.

## Dependencies and Teardown Order

Resources can depend on other resources. When tearing down a deployment, Skyr respects these dependencies:

```scl
let image = Container.Image({ name: "app", context: "." })
let pod = Container.Pod({ name: "app" })
let container = pod.Container({ name: "app", image: image.fullname })
```

The container depends on both the pod and the image. During teardown:
1. The container is destroyed first
2. Then the pod
3. Then the image

A resource won't be destroyed until all resources that depend on it are gone.

## Viewing Deployment Status

Use the CLI to check on your deployments:

```bash
skyr deployments list alice/my-app   # List all deployments
skyr deployments logs alice/my-app   # Stream deployment logs in real time
```

Or use the GraphQL API to query deployment status, view logs, and access artifacts.

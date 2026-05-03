# Deployments

Deployments are the core unit of infrastructure in Skyr. When you push code to a Skyr repository, Skyr creates a deployment and begins rolling out your infrastructure.

## Environments and Deployments

Skyr organizes infrastructure using a four-level deployment hierarchy: **Organization** → **Repository** → **Environment** → **Deployment**. Each resource within a deployment additionally carries a **region** (the metro it lives in) as part of its identity.

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

The deployment stays in this state until it either converges (transitions to Up) or is superseded by a new push.

### Up

Once a Desired deployment has fully converged — meaning its configuration has been evaluated with no new resource changes — Skyr checks whether any of its resources are **volatile**. If none are, the deployment transitions to the **Up** state.

An Up deployment is still the active deployment for its environment, but Skyr no longer re-evaluates it on each reconciliation cycle. This avoids unnecessary work for deployments that consist entirely of stable resources like random numbers, crypto keys, or artifacts.

If any resource is volatile (e.g., pods or containers), the deployment remains Desired and continues to be reconciled, since volatile resources may change or disappear externally.

When you push a new commit, an Up deployment is superseded just like a Desired one — it transitions to Lingering and follows the normal rollout process.

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
  main → commit A (Desired or Up)

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
import Std/Container

let image = Container.Image({ name: "app", context: ".", containerfile: "Containerfile" })
let pod = Container.Pod({ name: "app", containers: [{ image: image.fullname }] })
let httpPort = pod.Port({ port: 8080 })
```

The port depends on the pod, and the pod depends on the image. During teardown:
1. The port is destroyed first
2. Then the pod
3. Then the image

A resource won't be destroyed until all resources that depend on it are gone.

## Volatile Resources

Resources in Skyr can be marked as **volatile** by their plugin. A volatile resource represents external state that may change or disappear independently of Skyr — for example, a running container or pod that could be restarted, evicted, or destroyed by an external system.

Non-volatile resources are stable data that, once created, won't change unless Skyr explicitly updates them. Examples include random numbers, cryptographic keys, and build artifacts.

The distinction affects how Skyr manages deployments:

- A deployment with **only non-volatile resources** transitions to the Up state once converged, and is not re-evaluated until a new push.
- A deployment with **at least one volatile resource** remains in the Desired state and continues to be reconciled periodically, ensuring that volatile resources are kept in sync with the desired configuration.

The built-in resource types have the following volatility:

| Resource Type | Volatile |
|---------------|----------|
| `Std/Random.Int` | No |
| `Std/Time.Schedule` | Yes |
| `Std/Crypto.ED25519PrivateKey` | No |
| `Std/Crypto.ECDSAPrivateKey` | No |
| `Std/Crypto.RSAPrivateKey` | No |
| `Std/Crypto.CertificationRequest` | No |
| `Std/Crypto.CertificateSignature` | No |
| `Std/Artifact.File` | No |
| `Std/Container.Image` | No |
| `Std/Container.Pod` | Yes |
| `Std/Container.Pod.Port` | No |
| `Std/Container.Host` | No |
| `Std/Container.Host.Port` | No |

## Sticky Resources

Resources can be marked as **sticky** by their plugin. A sticky resource is not destroyed when its owning deployment becomes Undesired — it persists as a tombstone even after the deployment transitions to Down.

When a deployment is torn down, Skyr destroys all non-sticky owned resources as usual, but leaves sticky resources in place. The deployment transitions to Down once all its non-sticky resources have been cleaned up. The sticky resources remain in the resource database with their last owner, but since that deployment is Down, they are no longer actively reconciled or health-checked.

A resource can be both sticky and volatile. While its deployment is active (Desired), a sticky+volatile resource is reconciled normally. Once the deployment becomes Undesired and transitions to Down, the resource persists but is no longer repaired if it disappears externally.

None of the built-in resource types are currently sticky.

## Viewing Deployment Status

Use the CLI to check on your deployments:

From a working tree on the repo, org/repo come from the `skyr` remote (or `origin` as a fallback):

```bash
skyr deployments list                 # List all deployments
skyr deployments logs                 # Stream deployment logs in real time
skyr resources list                   # List all resources
skyr resources logs <resource-qid>    # Stream resource logs
```

Or pass `--org`/`--repo` explicitly to operate on a different repository.

# Cross-Repo Imports

Cross-repo imports let an SCL module in one Skyr repository depend on modules in *another* repository within the same organisation. This unlocks two complementary patterns:

- **Remote state** — read the outputs of resources owned by the foreign deployment, similar to Terraform's remote state. Useful for "I need the database hostname that the platform team's repo created."
- **Remote modules** — invoke resource-creating functions defined in the foreign repository. The resulting resource is owned by *your* deployment, similar to Terraform module reuse.

A single import path covers both. Whichever pattern applies to a given reference is determined by where the resource is created: in the foreign repo's own global expressions (remote state) or by a function the foreign repo exports and your repo calls (remote module).

## Declaring a dependency

Each repository declares its dependencies in a `Package.scle` file at the repo root:

```
import Std/Package

Package.Manifest

{
    dependencies: #{
        "MyOrg/Platform":     "main",
        "MyOrg/SharedLibs":   "tag:v1.2.0",
        "MyOrg/PinnedThing":  "b50d18287a6a3b86c3f45e3a973a389784d353dd",
    },
}
```

The `.scle` extension marks the file as **SCLE** — a self-contained SCL Expression format (one type expression followed by one value expression, plus any imports it needs). The manifest itself is a `Std/Package.Manifest` value.

Each entry in `dependencies` maps `Org/Repo` to a **specifier** that pins the dependency to a Git ref:

| Specifier syntax | Meaning |
| --- | --- |
| `"main"` *(any bare name)* | A branch — follow-the-channel, always resolves to that branch's currently-active deployment. |
| `"tag:v1.0.0"` | A tag — resolves to whichever deployment the tag points at. |
| `"b50d18287a..."` *(40 hex chars)* | A commit hash — fully deterministic pin, immune to upstream changes. |

Once the manifest exists, your SCL files can `import` modules from the declared repos:

```scl
import MyOrg/Platform/Database

let url = Database.primaryUrl
```

## Volatility tradeoff

Branch and tag specifiers are *volatile*: the foreign repo can advance without you re-pushing. Skyr's deployment engine reconciles your deployment on every cycle so foreign changes propagate automatically.

The cost: **a deployment with any volatile cross-repo pin stays in `Desired` forever**, never reaching the `Up` terminal state. The reconciliation loop keeps probing for foreign changes.

If you want your deployment to settle into `Up`, pin every dependency to a hash. The `skyr deps pin` workflow makes this convenient: develop against branches, then pin once you're ready to ship.

## Effect ownership

When you write:

```scl
import MyOrg/Platform/Storage

let bucket = Storage.makeBucket({ name: "my-app-data" })
```

the `makeBucket` function is *defined* in the platform repo, but you *invoke* it. The resulting bucket is owned by your deployment — it lives in your environment's resource namespace and follows your deployment's lifecycle. Updating the platform repo's `makeBucket` implementation, then redeploying your repo, will update *your* bucket.

By contrast, a top-level resource in the platform repo:

```scl
// In MyOrg/Platform's Main.scl
let primaryDb = Database.Postgres({ name: "primary" })
```

is owned by the platform deployment. When you read `Platform.primaryDb` from your repo, you read it as remote state — your deployment never tries to create or destroy it.

The general rule: **a resource is owned by the deployment whose code path most recently *chose* to invoke the resource call.** Function calls don't change ownership; only the deployment whose global expressions reach a resource call own it.

## Managing dependencies — `skyr deps`

The CLI exposes a small subcommand suite for editing `./Package.scle`:

```sh
skyr deps                        # list current deps
skyr deps add MyOrg/Repo main    # add or replace a dep
skyr deps rm MyOrg/Repo          # remove a dep
```

Manifest writes regenerate the file in a canonical form, so any comments in the original will be lost. If you prefer to edit manually, just open `Package.scle` in your editor — it has full Skyr LSP support (syntax diagnostics, formatting).

## Cross-region imports

Cross-region imports work transparently: an importer in `stockholm` can depend on a foreign repo whose home region is `paris`. SCS, the API edge, and the DE all resolve foreign repos through GDDB at compile time and read their CDB/RDB rows from the right region. The compiled module shape is what gets consumed — there is no runtime call across the region boundary just because two repos live apart.

Resources *created* by foreign-module calls obey the usual region rules: a region passed in the inputs (or inherited from the importer's repository) decides where the new resource is placed, regardless of where the foreign repo's home region is.

## v1 limitations

- **Cross-organisation imports are not supported.** Both the importer and dependency must be in the same organisation.
- **Diamond dependencies that resolve to *different* revisions of the same foreign repo are not supported.** A given foreign repo resolves to exactly one revision per deployment.
- **No CDB persistence of cross-deployment dependency edges yet.** The data is recorded in-memory during evaluation but isn't queryable across runs.
- **No "blocked on" UI cues.** A deployment stuck in `Desired` because of a pending foreign resource is observable through logs but not surfaced in the web UI.
- **`skyr deps update` and `skyr deps pin` are not yet implemented.** Both require server-side resolution of refs to commit hashes; they will land alongside the relevant API endpoints.

# Cross-Repo Imports — High-Level Design

## Motivation

Today, an SCL deployment is scoped to a single repository: `import` paths resolve against the standard library and files within the same repo. We want to extend SCL so that a module can import another module from a *different* repo in the same organization (and later, potentially across orgs), e.g.

```scl
import SameOrg/DifferentRepo/Module

let a = Module.makeResourceUsingRemoteFunction(...)   // Terraform-module reuse
let b = Module.resourceCreatedInOtherRepo             // Terraform-remote-state reuse
```

This gives us two complementary reuse patterns on a single mechanism:

- **Remote state**: resources created *by* the foreign deployment are visible to the local deployment as read-only outputs.
- **Remote modules**: functions exported by the foreign package, when invoked locally, create resources owned by the *local* deployment.

## Current Architecture Recap

Relevant pieces (all in `sclc`):

- `PackageFinder::find(raw_id) -> Arc<dyn Package>` resolves the first segments of an import path to a package.
- `Package::{lookup, load}` provide async access to SCL source files; `id()` names the segments the package owns.
- `DeploymentClient` in `cdb` already implements `Package` by reading git trees/blobs for a resolved `(org, repo, env, deployment)`.
- `CdbPackageFinder` already resolves `Org/Repo` → active `DeploymentClient`, but only for the local repo.
- `AsgEvaluator` walks the ASG in SCC order, evaluating **globals** and then bare **global expressions** (the only expressions that can emit resource effects). Function bodies are captured lazily as closures and executed only when called.
- `Effect::{CreateResource, UpdateResource, TouchResource}` carry a `ResourceId`, `inputs`, `dependencies`, and a `SourceTrace`. They are emitted through `EvalCtx::emit()` and drained by the DE, which turns them into RTQ messages.
- `RDB` stores the resource state for a `(env, deployment)` pair.

## Design Goals

1. **Ownership-correct effects.** Resources created by a foreign module's *own* global expressions remain owned by that foreign deployment. Resources created by *local* code paths — including local calls to functions imported from a foreign module — are owned by the local deployment.
2. **Remote state reads.** Outputs of already-created resources in the foreign deployment are readable during local evaluation without re-emitting effects.
3. **Deterministic version resolution.** A local deployment pins a specific revision of each imported foreign repo so that the dependency graph is stable for the life of the deployment.
4. **Access-controlled.** The UDB authorization layer must gate cross-repo access; a user cannot import a repo they can't read.
5. **No leakage through effects.** The DE must never apply, restore, or destroy a foreign-owned resource.

## Proposed Model

### 1. `PackageFinder` extended with cross-repo resolution

A new `CrossRepoPackageFinder` (or a generalization of `CdbPackageFinder`) resolves `Org/Repo` prefixes other than the local one. Given a raw module ID whose first two segments don't match the local deployment:

- Check UDB for the requesting user/org's access to `Org/Repo`.
- Resolve `Org/Repo` → `(env, deployment)` per the **version policy** (see §3).
- Return a cached `DeploymentClient` as the `Package`.

Stacking order: local package → cross-repo finder → stdlib.

### 2. Package identity carried through evaluation

Each `Package` already has a `PackageId`. We extend the evaluator so every value that *originated* from a foreign package knows its origin:

- Each `FnValue` closure records the `PackageId` (and, for remote packages, the remote deployment's QID) of the module that defined it.
- Each `GlobalNode`'s evaluation runs under an `EvalEnv` tagged with the defining package's QID.

The `EvalCtx` maintains a **current owner stack**. Entering a function body pushes the *caller's* owner (not the callee's — see §4). Global expressions are evaluated with the *defining* module's package as owner.

### 3. Version/commit/deployment selection

Inspired by GitHub Actions' ref-pinning model: the importer declares a **dependency manifest** (not a lockfile) at the repo root, mapping each foreign `Org/Repo` to a Git-ref-like specifier.

The manifest is expressed in **SCLE** (SCL Expression — see §3a), a new self-contained SCL value format analogous to JSON for JavaScript. The file lives at the repo root as `Package.scle`:

```
// Package.scle
import Std/Package

Package.Manifest

{
  dependencies: #{
    "SameOrg/DifferentRepo": "main",
    "SameOrg/LibraryRepo": "b50d18287a6a3b86c3f45e3a973a389784d353dd",
    "SameOrg/StagingTagged": "tag:v1.2.0",
  }
}
```

**Specifier conventions (string-typed):**

- Bare name → branch (e.g. `"main"`).
- `tag:<name>` → tag (matches the existing convention used for environment IDs).
- 40-character hex string → commit hash.

We deliberately use plain strings rather than tagged variants (SCL has no language-level variant concept) or records with optional fields (which would admit a degenerate "no specifier" case). A user who chooses to name a Git branch with 40 hex characters has bigger problems.

**`Std/Package` shape (v1):**

```
export type Manifest { dependencies: #{ Str: Str } }
```

The outer `Manifest` wrapper exists so we can add fields (`metadata`, `entry_point`, etc.) without breaking the schema. v1 only defines `dependencies`. The module lives at `crates/sclc/src/std/Package.scl` (type def) and `crates/sclc/src/std/package.rs` (extern registration), wired into `std_modules!` like any other stdlib module — even though there are no externs or resources to register, just a type.

Using SCLE buys us:

- **Strong, structural typing** of the manifest via `Std/Package.Manifest`, a stdlib type. Schema evolution is a normal SCL type evolution.
- **Familiar syntax** — no new lexer or grammar; users already know SCL.
- **Reuse of the existing SCLC pipeline** for parse/load/check (only the AST root differs).

**Validation timing.** SCS performs no manifest validation at push time. All validation — parse, type-check, dependency existence, access checks, branch resolution — happens in the DE during compile. A malformed or unresolvable manifest produces a deployment failure, indistinguishable from any other static SCL error. This keeps SCS minimal and means transient issues (e.g., a referenced branch not yet pushed in the dependency repo) self-heal without requiring the dependent to re-push.

Specifier forms:

- **Branch name** (e.g. `"main"`) — follow-the-channel: always resolves to the currently-active deployment of that environment. This is the correct choice when the importer wants live remote-state semantics (cross-deployment DAG with automatic propagation on supersession).
- **Tag** (e.g. `"v1.2.0"`) — human-readable stable version; resolves to whichever deployment the tag currently points at. Useful for "library-style" reuse with controlled upgrades.
- **Commit hash** — fully deterministic pin, immune to foreign repo changes.

No lockfile in v1. The manifest itself is the source of truth; branch/tag specifiers are deliberately dynamic. If a user wants reproducibility, they pin to a hash.

The manifest is part of the importer's repo content, so it is naturally versioned alongside the SCL code. "Upgrading" a dependency is either an edit to the manifest (for tags/hashes) or automatic (for branches).

**CLI dependency management.** A `skyr deps` subcommand suite is in scope for v1:

```sh
skyr deps                          # list current deps with their resolved state
skyr deps add <Org/Repo> <spec>    # add a new dep
skyr deps rm <Org/Repo>            # remove a dep
skyr deps update <Org/Repo>        # bump a hash-pinned dep to its current head
skyr deps update --all             # bump all hash-pinned deps
skyr deps pin <Org/Repo>           # resolve an existing ref pin to its current hash, freezing it
```

The commands operate on **direct dependencies only**; transitive pins are managed by each repo's own maintainers.

`update` and `pin` are intentionally separate verbs: `update` only touches deps that are *already* hash-pinned (it tracks the pin forward), while `pin` is a one-time operation that converts a branch/tag dep into a hash dep. This keeps each command's behavior unsurprising.

Manifest rewriting reuses the SCL formatter (extended to support SCLE). Because SCLE is a full expression language, the dependency map can in principle be any expression that evaluates to the right type — but the CLI only supports the trivial case: top-level record expression, with an inline `dependencies` dict literal, with literal key/value entries. If the manifest doesn't fit that shape, the CLI gives up with a clear error rather than attempting to rewrite a non-trivial expression.

**Transitive resolution is recursive.** When A imports B, the compiler reads *B's* manifest at the resolved B revision to resolve B's imports. A's manifest lists only A's direct dependencies; it does not need to (and cannot) override B's transitive pins. Rationale: encapsulation — each repo owns its own dependency declarations, matching Skyr's Git-native posture.

**Diamond dependencies are allowed.** If A→B→D and A→C→D resolve D to *different* versions (branch vs tag, or two different hashes), both versions coexist in the compiled ASG. This is safe because:

- SCL's type system is **fully structural**, so two structurally-compatible types from different D versions interoperate without nominal friction.
- SCL has no mutation, so there is no intended global shared state that could be fragmented by version duplication.

**Consequence for the ASG/runtime: the module identifier must include the resolved revision** (commit hash) so that `D@v1` and `D@v2` are distinct modules with distinct `RawModuleId`s, distinct globals, and distinct resource IDs. This is a concrete, non-negotiable constraint on the implementation.

### 3a. SCLE — SCL Expression format

SCLE is a self-contained SCL value format introduced as part of this work to give the manifest a sensible home (and to give Skyr a reusable "JSON-equivalent" for future config artifacts).

**Grammar:**

```
scle_mod ::= import_stmt* type_expr expr
```

— a sequence of imports, followed by a single type expression (the expected type of the value), followed by a single expression (the value itself). All three nonterminals are reused unchanged from the existing SCL grammar.

**Pipeline:**

1. New AST root `ScleMod` hooked into the SCLC compiler the same way the REPL entry point is.
2. Loader phase resolves the imports normally — including stdlib and (in principle) cross-repo. `Package.scle` is *expected* to import only `Std/...` modules to avoid a bootstrap circularity (the manifest is what enables cross-repo imports). We do not statically enforce this; we let it fail at resolution time if a programmer tries to do something silly.
3. Type checker runs `check_expr` on the body expression against the declared type expression.
4. Evaluator evaluates the body expression to a single value.

**Example (illustrative, not the manifest):**

```
// Example.scle
import Std/Option

{
  hello: Str
}

let f = fn(param: Bool)
  if (param) nil
  else 123;
{
  hello:
    let num = Option.map(f(true), fn(v) v * 2);
    "message {num}"
}
```

evaluates to `{ hello: "message nil" }`.

**Other future uses for SCLE:** any place where Skyr needs a strongly-typed, declarative configuration value (artifact metadata, environment configuration overrides, exported values from a deployment, etc.).

### 3b. Manifest resolution mechanics

**Consequences for the compiler/DE:**

- The `CrossRepoPackageFinder` reads the manifest up front and resolves each `(Org/Repo, specifier)` to a concrete `DeploymentQid` at compile time.
- For branch and tag specifiers, "active" follows SCS-style ref-tracking semantics: the ref either exists (resolves to its currently-desired deployment) or it doesn't (resolves to a runtime error in the dependent deployment). There is *no* fallback to Lingering — RDB scoping on environment means the new desired deployment naturally adopts existing resources, and a brief pending window during rollover is the expected, correct behavior.
- For hash specifiers, the resolution is to the specific commit's deployment. Commits are persistent forever (mirroring SCS), so this resolution is always stable. This is the typical "library-style" pin where remote-state volatility is undesired.

### 4. Effect ownership propagation

This is the central mechanic. The rule:

> A resource effect is owned by the package of the **first non-foreign stack frame** above the resource call. Equivalently: the owner is the package that most recently *chose* to invoke the code path that produced the effect.

Concretely:

- **Global expression in module M** → owner = M's package. (M *chose* to run this when the deployment loaded.)
- **Function `f` defined in module M, called from module N's global expression** → owner = N's package. (N chose to call `f`.)
- **Function `f` calls function `g` defined in module M'** → owner is unchanged; it's still whoever invoked `f`.

Implementation sketch: `EvalCtx` holds an `owner: PackageQid` that is mutated only when entering a **global expression**. Function calls do *not* change the owner. (`FnValue` closures capture their *definition-site* package for other purposes — e.g. name resolution — but **not** for effect ownership.)

**Closures are ownership-transparent.** Whoever *invokes* a closure owns the effects, regardless of where the closure was defined. There is deliberately no construct for "a closure whose invocation produces foreign-owned resources" — the SCL model has no use case for lazily creating foreign-deployment resources, and introducing one would muddy the effect-dropping logic in the DE.

**Resources referenced by value** across the boundary (e.g. local code reads `B.x` where `x` was created by B's own global expression) behave as remote state: B already emitted the effect when B's deployment ran; the local evaluation only reads outputs from B's RDB namespace.

Every emitted `Effect` is tagged with `owner: PackageQid` (new field). The DE filters:

- Effects whose owner matches the local deployment → enqueue as RTQ messages as today.
- Effects whose owner is foreign → **drop**, with the expectation that the foreign deployment (now or in the past) has already emitted the same effect on its own behalf. The resource is only *read* locally.

### 5. Remote state reads

Skyr's execution model treats resources as *emergent* from program evaluation: there is no first-class notion of "this resource exists / has been destroyed." A resource value is either materialized (concrete outputs available) or `<pending>` (not yet known). Cross-repo reads slot into this model directly:

- **Foreign resource is materialized in the remote RDB.** The evaluator, when it encounters a foreign-owned resource call (e.g. M's global `let a = Random.Int(...)` reached through L's `import M`), looks up the resource state in the foreign environment's RDB namespace (keyed by foreign **environment** QID — RDB is environment-scoped, not deployment-scoped, which is exactly what makes adoption work) and injects the concrete outputs into local evaluation.
- **Foreign resource is not yet materialized.** The value is `<pending>` locally. The local DE *still emits* the corresponding foreign-owned effect during evaluation, but it is dropped (foreign owner ≠ local). Local code that doesn't actually depend on the pending value continues to evaluate normally.

**Pending values propagate through closures naturally.** If `M.makeB = fn() Random.Int({ min: a.result, ... })` is invoked locally and `a` is `<pending>`, then the locally-owned resource produced by `makeB()` is itself `<pending>`. The local DE has no concrete inputs to emit a `Create`/`Update` for, so it simply does not — and the local deployment stays in `Desired`, waiting for the next reconciliation pass to find `a` materialized.

This gives us a powerful **time-independence** property: a user may deploy L *before* M (or before M reaches a state that produces `a`), and reconciliation will resolve the dependency once M catches up. There is no ordering constraint between deployments; pending-propagation plus reconciliation handles it.

Every foreign read is *also* recorded as a cross-deployment dependency (see §6a), primarily for observability and future change-driven scheduling.

**Operator visibility (important).** A deployment can be stuck `<pending>` on a foreign resource indefinitely. We must surface this clearly:

- The API/web should show, for each pending local resource, *which* foreign resource(s) it is blocked on — `M@main::Std/Random:x` not yet materialized.
- The cross-deployment DAG view (Observability section) should annotate edges with "blocking" status when an upstream is unmaterialized.
- LDB log entries should record pending-propagation events at info level so users can grep for "why is my deployment stuck."

### 6. `Effect` enum change

Add `owner: DeploymentQid` to each variant of `Effect`. The DE's effect drain loop filters on this field before enqueuing.

**`ResourceId` is environment-scoped, not global.** There is no cross-environment collision to worry about: `L::main::Std/Random:x` and `M::main::Std/Random:x` are distinct resources by construction, each stored in its own RDB namespace (the namespace is the environment QID — *not* the deployment QID; this is also why adoption works during supersession). Ownership on the `Effect` is *not* a disambiguator for identity — it simply tells the DE which environment's namespace the effect belongs to, and therefore whether the local DE should act on it (owner = local) or drop it (owner = foreign).

Consequence for upgrade stability: when `L` bumps `M` from `v1.0` to `v1.1` and `M.makeBucket` still produces a resource with the same plugin + same `name`, the `ResourceId` within `L@main` is unchanged and the existing resource is adopted (Touch/Update), not destroyed+recreated. This falls out of existing semantics without any special treatment. No call-stack–based hashing is needed.

### 6a. Cross-deployment resource dependencies

When local evaluation reads a foreign resource's outputs (remote-state semantics), a **cross-deployment dependency edge** is established: `L@main::b` depends on `M@main::a`. This must be represented explicitly because a branch-pinned dependency is *implicitly volatile* — M can redeploy without L redeploying, and L's resource must react.

**Proposed mechanics:**

- The evaluator records foreign-resource reads as first-class dependencies in the local `Effect` alongside the existing local `dependencies: Vec<ResourceId>`. Shape: `foreign_dependencies: Vec<(EnvironmentQid, ResourceId)>` — fully qualified by foreign environment.
- The DE persists these cross-environment edges so the dependency graph survives restarts and is queryable for the cross-deployment DAG view.
- **No explicit subscription mechanism is needed for v1.** Skyr's DE already runs a Kubernetes-style reconciliation loop for any deployment containing volatile resources; cross-repo deps simply ride that loop. Each reconciliation pass re-resolves the manifest's branch/tag pins, re-reads the foreign deployment's RDB, and re-evaluates — producing `Update`/`Touch` effects through the normal pipeline whenever upstream outputs have shifted.

**Volatility expectations (user-facing):**

- Branch specifier (`"main"`) → remote resources are **volatile**; local resources reading them may be re-applied at any time as the foreign environment advances.
- Tag specifier → also treated as volatile by the DE (a tag *can* be moved). For v1 we don't try to detect "this tag has not moved"; we just reconcile.
- Hash specifier → stable; not volatile by way of the manifest.

**Terminal-state rule (v1):** a deployment is only allowed to transition to `Up` (the terminal stable state) if it has **no volatile resources *and* no ref-pinned (branch/tag) cross-repo dependencies**. Any deployment that imports another repo via a branch or tag specifier stays in `Desired` and is reconciled on the existing schedule, even if all its own resources are intrinsically stable.

A future improvement is to allow such deployments to reach `Up` and be transitioned back to `Desired` *only* when an upstream change is detected — but this requires the explicit-subscription machinery deferred above and is out of scope for v1.

This needs to be clear in docs so users understand the tradeoff when picking a specifier: "branch/tag pins keep your deployment in the reconciliation loop; hash pins let it settle."

**The local-creates-via-remote-function case** (e.g. `let b = M.makeB()` in L, where `makeB` reads foreign state `M.a`): the resource `b` is owned by L, lives at `L@main::Std/Random:x`, and carries a `foreign_dependency` on `M@main::Std/Random:x`. When M redeploys and `a.result` changes, L's DE sees the upstream change and re-evaluates `b` — which may produce an `UpdateResource` effect that flows through the normal local RTQ pipeline. This is the coherent, intended behavior.

### 7. Cross-repo access control

**v1 rule: implicit read access within the same organization.** Any deployment in org `X` may read any other repo in org `X`. Cross-org imports are disallowed in v1.

Access is checked at **load time** by `CrossRepoPackageFinder::find()`, so violations fail the compile early with a clear diagnostic rather than mid-evaluation. On failure, return a generic "Permission denied" error (per project convention).

Importantly, no deployment ever needs **write access** to another repo's resource namespace: effect ownership is determined by the evaluator, and foreign-owned effects are dropped locally rather than written to a foreign RDB namespace.

**Principal identity (future work).** Today, access decisions are tied to the user who pushed the commit. A deployment should instead act as its own principal identity — independent of the committing user — so that ACLs can be granted to deployments, not people. This is out of scope for v1 (when we're using implicit same-org access), but the design should not *foreclose* it: the load-time access check should be framed as "deployment D1 may read repo R" rather than "user U may read repo R," so that when deployment principals land, only the resolution of the principal changes.

## Observability

**v1 scope: data model only — no dedicated UI surface.**

- Each foreign package load is logged to LDB with the resolved foreign deployment QID.
- Effects dropped due to foreign ownership are logged (at debug level) so operators can audit what *would have been* emitted.
- Cross-environment dependency edges (see §6a) are persisted alongside the local deployment so that the data is queryable, even though no first-class UI consumes it yet.

**Deferred to later milestones:**

- Per-deployment "imports" view in the web UI showing foreign deps with resolution + materialization state.
- Reverse index ("who imports me?") for upstream deployments — useful for impact analysis before redeploying a popular library, but adds a write-amplification step.
- Explicit "blocked on pending foreign resource X" UI cues for deployments stuck in `Desired`.

These are good ideas, but they aren't required for the underlying mechanism to work; users in v1 can fall back to logs and the persisted edge data.

## Open Questions

1. **Reconciliation cost at scale.** Every deployment with a ref-pinned cross-repo dependency stays in the reconciliation loop indefinitely. With many such deployments and a branch-pinned manifest fan-in to a popular library repo, the steady-state load on CDB/RDB could be significant. Do we need to budget for this (e.g. backoff, batching), or is the existing reconciliation cadence already cheap enough?
2. **Compile-time type visibility.** Resolved: full source. The DE has access to the entire CDB-resident universe of packages within a Skyr instance (cross-instance access is explicitly out of scope), and the SCLC Loader's existing import-graph spidering pulls only the sources actually reached. The cost is acceptable; an interface-artifact cache can be introduced later as a transparent optimization in front of `CrossRepoPackageFinder` if profiling warrants it.
3. **Destroy semantics.** Resolved: no special machinery needed. Foreign-owned effects are dropped locally so foreign teardown is never the local DE's concern; local-owned resources created via a remote function live in the local namespace and follow the existing supersession/lingering pipeline regardless of whether the originating import is still present in a newer version of L.
4. **Cycle detection.** Resolved by the existing ASG SCC check, which extends to cross-repo for free. Module-level cycles (A imports B imports A) are *allowed* — the ASG admits cyclic edges. The constraint is on globals: an SCC of globals is only legal if every global in it is a function, since function values can be evaluated lazily and don't form an evaluation cycle. Any non-function global in an SCC is a compile error. Therefore no resource value can ever depend on itself, transitively or otherwise — including across cross-repo boundaries — because resources are non-function globals and would fail the SCC check at compile time. There is no separate "runtime cross-deployment cycle" failure mode to handle.
5. **Plugin availability.** Deferred. v1 assumes plugin availability is uniform across all reachable environments (a single Skyr cluster running a known set of stdlib plugins). Revisit when third-party plugins or extern mechanisms land — at that point we may need a pre-evaluation check that walks the ASG for plugin references reachable from local-owned globals.

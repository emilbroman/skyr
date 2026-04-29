# Rust Style

Conventions for writing Rust code in Skyr. Broad topic — captured incrementally.

## Error Handling

The default depends on whether the crate forms a public API or owns its `main`.

- **Lib crates** (e.g., `cdb`, `rdb`, `sclc`, `ids`) form a public API and shouldn't make assumptions about their consumers' environment. Use **custom error enums via `thiserror`**, exposed as part of the crate's API.
- **Crates that own their `main`** (daemons, CLIs) use **`anyhow`** as the conventional pick. Errors are caught and reported; callers don't programmatically inspect them.

Rule of thumb when in doubt: would a downstream caller want to match on the error variant? If yes, `thiserror`. If no, `anyhow`.

## Async vs Sync

Function coloring is handled by **crossing the bridge when you get there**:

- Make a function **sync as long as it doesn't require I/O**.
- **Refactor to async later** if it ends up needing I/O.

There is no rule that "if most callers are async, the callee must be async too." Sync helpers are fine in async code; promote them only when the helper itself starts to need I/O.

### Runtime

**Tokio** is the runtime — somewhat arbitrarily chosen, but consistency is the value. Use the same surface API (`tokio::spawn`, `tokio::sync`, `tokio::net`, etc.) everywhere unless there's a concrete reason to do otherwise.

## Newtypes and Typed Wrappers

Newtype discipline is not confined to identity types. The `ids` crate (`OrgId`, `RepoQid`, `EnvironmentQid`, `DeploymentQid`) is the most visible example, but the pattern applies to **other domain values too** — hashes, timestamps, durations, byte counts, paths, etc.

### When a wrapper already exists

If a primitive value is used **in place of a wrapper that already exists**, that's an issue and should be addressed.

Parsing an ID/QID just to re-serialize it later is a cheap price to pay for knowing we are not mixing up different kinds of IDs. The type system carrying the meaning across function boundaries is worth more than saving a parse.

### When no wrapper exists yet

For domain values that don't yet have a newtype, **using the raw type is a code smell, not a bug**. It doesn't force extraction on its own. Wrap it when it starts paying its way — typically once the same primitive shows up in multiple places carrying the same meaning, or once a function signature becomes ambiguous about what the primitive represents.

### Triggers to consider (not hard rules)

A wrapper becomes more justified when one or more of these apply, but treat each as a **point of consideration**, not a hard rule:

- The value crosses a crate boundary.
- The value appears in a public API.
- More than one variant of the same primitive shape exists in the codebase, and they could be confused.

## Panics: `unwrap` / `expect` / `panic!`

The rule splits cleanly between lib crates and binaries.

### Lib crates — avoid panics like the plague

Libraries can't know how their consumers want to handle failure. Let-it-crash may or may not be appropriate at the workload level, but **the library is in no position to decide that for the workload**. So in lib crates, **never panic**: no `unwrap`, no `expect`, no `panic!`, no array indexing that could go out of bounds, etc. Surface errors through the crate's `thiserror` enum and let the caller decide.

### Binaries — case by case

In crates that own their `main`, `unwrap` / `expect` / `panic!` are acceptable when **let-it-crash is a deliberate part of how the workload is built**. If the daemon is designed to be restarted on failure (orchestrator-managed, idempotent on resume, no cleanup-on-crash work owed), letting a thread or process crash on an unexpected condition is a legitimate strategy.

Outside that paradigm, treat panics with the same caution as in lib crates. There's no global rule — judge per binary, per code path.

## Visibility and Public API Surface

### Default to `pub(crate)`

Use **`pub(crate)`** until something genuinely needs to be part of the crate's intended public API. Don't pre-emptively mark items `pub` because they "might" be needed externally — promote them when the use case actually shows up.

### Flat re-exports preferred

Curate the crate's public API at the **crate root** with flat re-exports. Callers should be able to write `use cdb::Foo` rather than `use cdb::deployments::Foo`. The internal module structure is an implementation detail; the re-export layer is the contract.

### No backdoors across crates

If something is not part of a crate's public API, it must not be used from other crates. Period.

There is no `*-internal` crate pattern, no "`pub` but please don't use it" convention. The rule is simple: cross-crate usage means the item is part of the public API and is committed to as such. If a helper is needed by a sibling crate, either:

- Promote it to the public API (intentionally, with naming and shape that fits public consumption), or
- Move the consumer's code into the owning crate, or
- Duplicate the helper.

But do not punch a hole through the boundary.

## Trait Objects vs Generics

Strictly case-by-case. There is no default reach.

Trait objects (`Box<dyn Trait>`, `&dyn Trait`) are **allowed but not preferred**. They earn their place when **the values are passed around a lot**, where the type-parameter propagation of generics would be more painful than the vtable cost. Compiler diagnostics are an example of this — they flow through enough call sites that monomorphizing on the concrete diagnostic type would be more burden than benefit.

For most other code, generics with trait bounds or `impl Trait` are the natural fit. Pick whichever reads best at the call site and propagate the type parameter only as far as it actually needs to go.

## Ownership at API Boundaries

Decide based on what the function actually does with the value:

- **Doesn't need to copy → take by reference.** `&str`, `&[T]`, `&Foo`. Always. Don't take an owned value just because the caller might happen to have one.
- **Needs to copy → take `impl Into<T>` / `impl ToString` / similar.** This pushes the conversion (and any allocation) to the boundary, lets callers pass either an owned or borrowed value naturally, and keeps the function body working with the concrete owned type.
- **Explicitly taking ownership (and not just to clone) → take plain owned `T`.** Use this when the function's contract is "I'm taking this from you" — for example, moving a value into a long-lived struct, sending it across a channel, or consuming it irreversibly. The plain owned signature signals that intent.

The middle case is the nuance: `impl Into` is for "I need a copy"; plain owned `T` is for "I'm taking yours." Don't conflate them — the signature is part of the documentation.

## Module Organization

### Filename style by default, mod.rs on demand

Use **filename style** (`src/foo.rs`) for modules. Only when a module's own contents start needing to be split across multiple files, convert it: replace `src/foo.rs` with `src/foo/mod.rs` and extract the pieces into sibling files under `src/foo/`.

Don't pre-emptively create `src/foo/mod.rs` for an empty or single-file module.

### lib.rs is either everything or only the seam

`lib.rs` is allowed to carry **meaningful code** — but only as long as it is the **only** source of code in the crate. The moment the crate splits into more than one module, `lib.rs` flips role: it becomes the re-export and high-level-structure seam, and the meaningful code moves out into its own modules.

**Don't mix.** Either:

- `lib.rs` only, and it carries everything, *or*
- `lib.rs` curates the public API (re-exports, top-level structure, docs) while real code lives in sibling modules.

The half-and-half state — meaningful logic in `lib.rs` *plus* other modules — is the configuration to avoid.

## Macros

Don't use macros excessively. They're not a default tool.

The case where they earn their keep is **avoiding tedious repetition**, and in particular **avoiding situations where the same identical list must be repeated in multiple places**. Repeated lists are highly likely to fall out of sync when one of the cases is missed, and that desync is usually a correctness bug rather than a stylistic blemish.

Macros are specifically the right tool when their job is to **prevent tightly-coupled listings from going out of sync**. `std_modules!` and `test_case!` in this codebase are examples of that pattern.

If a macro is being introduced for any other reason (concise syntax, ergonomic shorthand, custom DSL), think twice — most of those cases are better served by functions, traits, or generics.

## Doc Comments

Case by case. **Comments are not mandated** — neither `///` on public items nor `//` on internals. Names and types should carry most of the documentation weight on their own.

Where a comment genuinely helps — explaining a non-obvious invariant, a subtle interaction, or a design decision that isn't visible from the signature — include it. The bar is "this would help a future reader understand something they couldn't get from the code alone."

When you do write a comment, **keep it up to date**. A stale comment is worse than no comment at all.

## Derive Macros

**Don't include any `#[derive(...)]` without a concrete reason to do so.** No defensive `#[derive(Debug, Clone, PartialEq, Eq, Hash)]` on a fresh struct just in case.

Skyr is a monorepo. If a derive turns out to be needed later — even by a different crate — it can be added then. There is no external consumer we're protecting against, and no semver story that punishes adding a derive after the fact. So the default is: add derives as the actual call sites demand them, not in advance.

## Conversions and Std Trait Impls

**Prefer conventional std trait impls** for things that match a std trait's contract — `From` / `TryFrom` for conversions, `Display` for human-readable rendering, `FromStr` for parsing, `Default` for zero-values, `AsRef` / `Borrow` for cheap views, etc.

A non-trait method or associated function whose signature and semantics fit a std trait — `Foo::from_bar(b)` instead of `impl From<Bar> for Foo`, or `foo.to_string_repr()` instead of `impl Display` — is a **code smell**. Either implement the trait, or there's a reason the trait isn't the right fit (and that reason should be visible in the design).

## Copying and Lifetimes (general rule)

Be mindful of excessive copying, but **not at the cost of API simplicity**. A clean signature that clones once is usually preferable to a tangled signature that avoids the clone.

Lifetimes — especially **type-level lifetimes on structs/enums** — should be introduced only for a good reason. Threading a `'a` through a public type pulls every consumer into the lifetime story; it should pay for itself in concrete copy avoidance, not in hypothetical performance.

## Iterators and Collections

- **Prefer iterator pipelining over explicit loops.** `.iter().filter().map()...` reads better than the equivalent `for` loop with mutation in most cases.
- **Treat `.collect::<Vec<_>>()` as a potential cost.** Materializing an intermediate `Vec` is potentially very expensive and should be seen as a point of improvement when it shows up in hot paths or in places where the consumer would have been happy with an iterator.
- When a function produces a sequence and there's no reason to materialize, prefer returning `impl Iterator<Item = T>` and let the caller decide. Materialize when there's a concrete reason to (multiple passes, indexing, ownership transfer, API simplicity per the rule above).

## Shared Ownership and Synchronization

### Escalate when there's a need

The hierarchy is:

1. **Simple borrow** is always better than a smart pointer.
2. **Smart pointer (`Arc<T>`)** is often — not always — better than a *non-simple* borrow.

Don't reach for `Arc` pre-emptively. Start with ownership + borrow, and escalate when the lifetime story actually starts fighting you. The break-even point is when the borrow becomes more contorted than an `Arc` would be.

### Async primitives in async code

When in async context, **never use `std::sync` blocking synchronization primitives** if there is an async counterpart available. Use `tokio::sync::Mutex` / `tokio::sync::RwLock` / `tokio::sync::Notify` / `tokio::sync` channels rather than the `std::sync` equivalents.

The exception is "a really good reason" — e.g., a critical section so short that the blocking primitive is provably better, or an interface that requires `Send + Sync` on a `std::sync` type. Those exceptions exist; they should be the exception, not the default.

## Logging and Tracing

### Use `tracing`, nothing else

**Everyone uses `tracing`.** Never `log`, never `env_logger`, never any other logging crate. The whole codebase speaks one logging API.

- **Binary crates** are responsible for the full setup ceremony: `tracing_subscriber` initialization, `RUST_LOG` env loading, formatter configuration, etc.
- **Lib crates** assume the subscriber is set up. They just emit `tracing::info!` / `tracing::debug!` / etc. and don't touch subscriber state.

### Log liberally, but not excessively or redundantly

Sprinkle log statements where they will help understanding the system's behavior. Don't be stingy — observability is part of the design.

But don't be repetitive. Two log lines that say the same thing from adjacent call sites is noise, not signal. Pick the one that's at the right layer of abstraction and drop the other.

### Spans

We don't currently have tooling set up to take advantage of `tracing` spans (`#[instrument]`, `tracing::span!`, etc.). **Avoid spans for now.** When the tooling exists, the guideline can be revisited.

## Testing

No hard guidelines — case-by-case. The general bias is toward the **smallest footprint that does the job**:

- **Inline `#[cfg(test)] mod tests`, colocated with the code under test, by default.**
- **Standalone `tests/` integration directories** *if there's a need* — i.e., when the test exercises the crate's public API as an external consumer would, or when integration setup doesn't fit inside a unit test.
- **Fixture-based tests** only when the structure provides genuine value. The `sclc` compiler test fixtures (`crates/sclc/src/tests/`) are a clear example: each fixture is a self-contained SCL program with expected exports/effects/diagnostics, and that shape is worth its weight there. Don't reach for fixtures for things that fit cleanly in inline unit tests.

## External Dependencies

Pulling in a `crates.io` dependency is a **liability**, so do it with caution. The single most important factor: **avoid dependencies that are likely to change**, since they will drag the codebase along with them.

The categories that earn their keep:

- **Spec-based crates** are generally safe to depend on — the spec is the contract, and the crate is just an implementation of it. Examples: `sha2`, `base64`, anything implementing a stable algorithm or wire format.
- **Conventional / popular crates for higher-level core concerns** — the ecosystem standards. Examples: `reqwest`, `serde`, `thiserror`, `anyhow`, `tokio`. These are well-maintained, broadly used, and not going to disappear.

The category to avoid:

- **Crates whose transitive dependency footprint is larger than the value they provide.** A small convenience that drags in a big tree of unfamiliar dependencies is usually a bad trade — write it yourself or use a leaner alternative.

## `unsafe`

**Hard disallow in our own code.** The Skyr codebase is safe Rust.

This rule does *not* extend to dependencies — they're allowed to use `unsafe` internally; that's their problem to get right.

The only escape hatch is highly specific edge cases where safe Rust genuinely cannot express what's needed. In that case:

- **Document closely.** Every `unsafe` block needs a clear `// SAFETY:` comment explaining the invariants and why they hold.
- **Rule out safe alternatives explicitly.** The justification should make clear *why* safe Rust isn't sufficient — not just that `unsafe` is more convenient.

If those bars aren't both met, it's not an edge case, and the answer is "use safe Rust."

## Clippy and Lint Posture

The bar enforced in CI (and in `CLAUDE.md`) is `cargo clippy --workspace -- -D warnings` — i.e., the **default `warn` set, all denied**. We don't opt into `clippy::pedantic` or `clippy::nursery` globally, and there's no curated list of extra lints to turn on.

### `#[allow(...)]` requires a real reason

`#[allow(some_lint)]` is fine when there's a genuine reason — and that reason must be written down inline as a comment. The bar for what counts as a reason is non-trivial:

- **Not acceptable:** sloppiness, "we'll fix this later", "I disagree with the lint in general."
- **Acceptable:** the lint is wrong about this specific case, the alternative is materially worse for readability or correctness, or there's a principled exception that the comment can articulate.

If you can't write down a defensible reason, the answer is to fix the warning, not to suppress it.

## Performance Posture

**Be deliberate and performance-aware, but not at the cost of readability or maintainability.** The default is to write code that is reasonable about allocations, cloning, and asymptotic complexity from the start — not to wait for a profiler to flag the obvious. But don't twist code into knots to shave milliseconds.

### No known hot paths today

There are **no code paths in Skyr today that warrant special performance treatment** beyond the general awareness above. The SCL evaluator, the RTE reconciliation loop, queue consumers, etc., are all written at the "deliberate but readable" level — none of them have been identified as hot enough to deserve extra care. If that changes, the affected paths can earn dedicated treatment.

### Measuring vs reasoning

"Obviously faster" is acceptable on inspection. Measuring (`criterion`, flamegraphs, etc.) is good when the answer isn't obvious or when the change is non-trivial — but it's not a precondition for routine perf-aware coding.

## Workspace Cargo Conventions

- **Dependencies are declared per-crate.** Each crate's `Cargo.toml` lists its own dependencies directly. We don't centralize through `[workspace.dependencies]` and pull via `workspace = true`.
- **Lints are workspace-level** when any customization is done. If a lint configuration is to be applied, it goes in the workspace root rather than being repeated per-crate.
- **Build-time optimizations are per-crate.** `[profile.*]` tweaks (LTO, codegen-units, panic strategy, opt-level overrides) live in the crate that needs them, not in a shared workspace profile.

## Serialization and Wire Formats

### `serde` is the default

Use `serde` for any in-Rust serialization need that isn't already covered by a specific protocol crate (e.g., `prost` for the gRPC protocols).

### Queue messages default to JSON

For queue payloads (RTQ, RQ, NQ), **JSON is the default** — its main virtue is that humans can read it directly when debugging and inspecting queue contents.

This is **case-by-case** if JSON ends up being too inflated in size for a particular message shape. Switch to a more compact format (e.g., a binary serde format) when the size cost is real and measured, not pre-emptively.

### Avoid DTO types

Don't introduce parallel DTO ("data transfer object") types that mirror the domain type just to carry the wire format. Derive `Serialize` / `Deserialize` directly on the domain type, and let the type *be* its own wire shape.

DTO layers tend to **make the architecture less understandable** — they double the surface area, force every change to be made in two places, and obscure the relationship between the domain model and what actually goes over the wire. Keep the domain type and the wire type unified unless there's a compelling reason to split them.

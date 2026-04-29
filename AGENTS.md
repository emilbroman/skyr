# Skyr Agent Notes

This file contains operational guidance for AI agents working on the Skyr codebase — checklists, project-specific procedures, and gotchas that are particular to Skyr.

For broader conventions and design principles, see:

- **[dev/guidelines/](dev/guidelines/index.md)** — architectural principles, boundaries, storage, naming, Rust style. Read these before making non-trivial design decisions.
- **[README](README.md)** and individual crate READMEs — architecture and crate descriptions.
- **[docs/](docs/index.md)** — end-user documentation.

### Before Committing

Always run formatting and linting before committing:

**Rust (from repo root):**

```sh
cargo clippy --workspace -- -D warnings
cargo test
cargo fmt
```

**Web frontend (from `web/`):**

```sh
npm run format        # biome format --write .
npx biome check .     # format + lint + import sorting (read-only)
npm run check         # svelte-kit sync && svelte-check
```

In CI, the web frontend runs `npx biome ci .` (which checks formatting, linting, and import sorting in one pass) followed by `npm run check` (svelte-check for type checking).

Fix any warnings or errors before pushing. See [Rust Style: Clippy and Lint Posture](dev/guidelines/rust-style.md#clippy-and-lint-posture) for the policy on warnings and `#[allow(...)]`.

### Web Frontend Formatting and Linting

The `web/` directory uses [Biome](https://biomejs.dev/) for formatting and linting, configured in `web/biome.json`.

**Formatting rules** (aligned with `cargo fmt` conventions):
- 4-space indentation
- 100-character line width
- Double quotes, trailing commas, semicolons always
- Parentheses always around arrow function parameters

**Biome + Svelte caveats:**
- Biome's Svelte support is experimental — it cannot see variables, imports, or functions used in Svelte template markup (`{...}` blocks, `<Component />` tags, event handlers, etc.).
- Because of this, `noUnusedVariables` is disabled globally, and `noUnusedImports`, `useImportType`, and `organizeImports` are disabled for `.svelte` files via overrides. Svelte-check handles these correctly.
- Do **not** run `biome lint --unsafe` on `.svelte` files — it will rename template-referenced variables with `_` prefixes and break the app.
- Do **not** convert `import { Foo }` to `import type { Foo }` in `.svelte` files if `Foo` is used as a value in the template (e.g., as an enum variant in comparisons). Biome thinks it's type-only because it can't see template usage.

### Conventions and Gotchas

- Keep deployment state transitions coherent across `scs` and `de`.
- When changing schema in `cdb`/`rdb`, update table creation + prepared statements together.
- In `sclc`, parse functions return `Diagnosed<Option<_>>` and report syntax errors via diagnostics instead of `Result<_, ParseError>`.
- In `scl`, the REPL ignores empty lines and uses `Diagnosed` reporting helpers for parse/type diagnostics.
- Whenever the GraphQL server is updated in a way that impacts the schema, regenerate `crates/api/schema.graphql` by running `cargo run -p api -- --write-schema`.
- When writing new RTP plugins, follow the pattern in `plugin_std_random` or `plugin_std_artifact`.
- For ADB operations, configure endpoint/bucket via CLI args or environment variables.
- For LDB logging, use `NamespacePublisher` with deployment QID as namespace.
- The `ids` crate defines the four-level namespace hierarchy (Org → Repo → Environment → Deployment). Namespace strings (for RDB, LDB, ADB) are QID `.to_string()` values — use environment QIDs for RDB namespaces and deployment QIDs for LDB/ADB namespaces. (See [Rust Style: Newtypes and Typed Wrappers](dev/guidelines/rust-style.md#newtypes-and-typed-wrappers) on using typed IDs/QIDs rather than raw strings.)
- Note: spelling is consistently `supersede/supersession` in schema/API names.
- READMEs and crate-level docs are **internal documentation** aimed at developers working on the codebase. The `docs/` directory contains **external documentation** aimed at end users. When making changes, update the relevant docs to reflect them — but internal-only changes (refactors, internal API changes, implementation details) should **not** be added to external docs.
- When adding new SCL language features (syntax, types, standard library modules/functions, etc.), update the corresponding end-user documentation in `docs/scl/`:
  - `docs/scl/syntax.md` — for new syntax constructs (operators, expressions, statements, keywords)
  - `docs/scl/types.md` — for type system changes (new types, subtyping rules, inference behavior)
  - `docs/scl/stdlib.md` — for new or changed standard library modules and functions
  - `docs/scl/index.md` — if the feature deserves a mention in the "Language Features at a Glance" section

### SCL Compiler and Specification

The SCL language has two sources of truth that must stay in sync: the reference implementation in `crates/sclc` and the formal specification under `spec/` (Typst chapters, compiled via `spec/main.typ` to `spec/scl-spec.pdf`).

- **Any syntactic or semantic change to the compiler must be accompanied by the corresponding change to the specification, in the same PR.** This includes (but is not limited to):
  - Grammar changes in `crates/sclc/src/parser.rs` → update `spec/ch02_lexical.typ` and/or `spec/ch03_syntax.typ`
  - New or changed type-checking rules (`crates/sclc/src/checker.rs`, `crates/sclc/src/ast/*.rs` `type_synth`/`type_check` impls) → update `spec/ch04_types.typ`, `spec/ch05_subtyping.typ`, `spec/ch06_propositions.typ`, or `spec/ch07_static.typ` as appropriate
  - Changes to evaluation behavior (`crates/sclc/src/eval.rs`, `ast/*.rs` `eval` impls) → update `spec/ch08_dynamic.typ`
  - Changes to module resolution, import semantics, or dependency analysis → update `spec/ch09_modules.typ`
- **Grammar changes must also be mirrored in every other grammar consumer in this repo**, or SCL source will render/parse incorrectly in editors, docs, and tooling:
  - `web/src/lib/scl.tmLanguage.json` — TextMate grammar used by the web frontend for syntax highlighting (docs, code blocks, any `.scl` rendering in the UI).
  - `crates/sclc/tree-sitter-scl/grammar.js` — tree-sitter grammar for SCL source files (editor tooling, incremental parsing).
  - `crates/sclc/tree-sitter-scle/grammar.js` — tree-sitter grammar for SCLE (the evaluated/serialized SCL form); update this only if the change affects the SCLE subset.
- **Conversely**, do not change the spec in isolation either — if the spec is wrong, either fix the spec to match the compiler *or* change the compiler to match the corrected spec, but do not leave them divergent.
- When the change is a bug fix that aligns the two, state in the commit message which direction the alignment went (e.g., "compiler now matches spec §8 rule E-Call" vs. "spec §4.1 corrected to match implementation").
- The spec is authoritative for *language* semantics; the implementation is authoritative for host/runtime details (stdlib extern bindings, resource effects, RDB formats). Chapter 10 (`ch10_stdlib.typ`) intentionally does **not** enumerate stdlib signatures — those live in `crates/sclc/src/std/*.scl` and its rendered form in `docs/scl/stdlib.md`.
- Changes to the end-user documentation in `docs/scl/` are separate from spec updates — see the `docs/scl/*` bullets above. Spec updates are for language designers and implementers; `docs/` is for SCL users.
- Before committing spec changes, render the PDF to make sure the Typst sources still compile: `make spec` (or `typst compile spec/main.typ spec/scl-spec.pdf` directly). `make spec-watch` is useful while iterating.

### Adding a New RTP Plugin

RTP plugins are the canonical example of "gRPC at a protocol boundary" — see [Architecture: Queue vs gRPC](dev/guidelines/architecture.md#queue-vs-grpc) and [Boundaries](dev/guidelines/boundaries.md) for the design rationale.

When adding a new standard library RTP plugin (e.g., `plugin_std_foo` for `Std/Foo`), update the following locations:

1. **Create the crate** — `crates/plugin_std_foo/` with `Cargo.toml` and `src/main.rs`. Follow the pattern in `plugin_std_random` (simple) or `plugin_std_artifact` (with external deps). Implement `rtp::Plugin` for create/update and optionally delete/check.
2. **SCL type definition** — add the resource function signature to the corresponding `.scl` file in `crates/sclc/src/std/` (e.g., `Foo.scl` for a new module, or an existing file like `Time.scl` for additions).
3. **Extern function registration** — add `register_extern` in the corresponding Rust module under `crates/sclc/src/std/`. Wire it into the `std_modules!` macro in `crates/sclc/src/std/mod.rs` if adding a new module.
4. **Workspace** — add the crate to `members` in the root `Cargo.toml`.
5. **Containerfile** — in `dev/Containerfile.skyr`:
   - `COPY` the new crate's `Cargo.toml` (planner stage)
   - Add to the `mkdir -p` command
   - Add a stub `printf 'fn main() {}\n'` line
   - Add to the `cargo build --release -p ...` command
   - Add to the `for bin in ...` artifact copy loop
   - Add a `COPY --from=build` line in the final image stage
6. **Compose** — in `dev/podman-compose.yml`:
   - Add a service definition with a unique port (check existing ports to avoid collisions)
   - Add `--plugin "Std/Foo@tcp://plugin-std-foo:<port>"` to every `rte-*` worker
7. **Makefile** — add the service name to the `up` target's `podman compose up` command.
8. **Terraform** — in `infra/skyr-k8s/services.tf`:
   - Add `--plugin "Std/Foo@unix://_/var/run/plugins/foo.sock"` to the RTE args (or `tcp://` for plugins that need their own Deployment, like Container)
   - Add a sidecar container definition (or a separate Deployment + Service in `plugins.tf` for complex plugins)
9. **CI workflow** — in `.github/workflows/ci.yml`:
   - Add to the `service-images` matrix `binary` list
   - Add the image reference to the release body
10. **Documentation**:
    - **Plugin README** — create `crates/plugin_std_foo/README.md` following the pattern in `plugin_std_random`
    - **Root README** — add a row to the Plugins table and to the Running Locally service table
    - **RTE README** — add the plugin to the example `--plugin` flags in `crates/rte/README.md`
    - **RTP README** — add the plugin to the Related Crates list in `crates/rtp/README.md`
    - **External docs** — add user-facing documentation in `docs/scl/stdlib.md` for the new resource types

### Adding Standard Library Tests

The compiler is the textbook case where fixture-based tests pay their keep — see [Rust Style: Testing](dev/guidelines/rust-style.md#testing) for the broader rule.

When adding or modifying standard library functions/modules in `sclc`, add fixture-based integration tests under `crates/sclc/src/tests/`:

1. **Create a fixture directory** — `crates/sclc/src/tests/TestName/` containing:
   - `Main.scl` (required) — the SCL source code to compile and evaluate
   - `exports.txt` (optional) — expected exported value, defaults to `{}`
   - `effects.log` (optional) — expected resource effects (`CreateResource`, `UpdateResource`, `TouchResource`), one per line
   - `rdb.json` (optional) — pre-existing resource state to simulate already-deployed resources
   - `diag.log` (optional) — expected diagnostic messages (type errors, undefined variables, etc.)

2. **Register the test** — add `test_case!(TestName);` in `crates/sclc/src/tests/mod.rs`.

3. **Test resource lifecycles** — for resource functions, write three tests:
   - **Create** (no `rdb.json`) — expects `CreateResource` effect and `<pending>` export
   - **Touch** (`rdb.json` with matching inputs) — expects `TouchResource` effect and concrete outputs
   - **Update** (`rdb.json` with different inputs) — expects `UpdateResource` effect and `<pending>` export

4. **Test pure/extern functions** — verify concrete computed values in `exports.txt`.

5. **Test type errors** — verify expected diagnostic messages in `diag.log`.

**Important notes:**
- Record fields in `exports.txt` are BTreeMap-ordered (alphabetical)
- SCL strings use `{` for interpolation — escape with `\{` when literal braces are needed
- Resource IDs for Crypto CSR/CertificateSignature are hash-based — run the test once with a placeholder to capture the actual hash

## Running Locally

See the [README](README.md#running-locally) for full service and port listings.

For manual testing:
- Build the CLI with `cargo build -p cli`
- Run `make up`
- Use the local `test-repo/` (gitignored) for Git server tests; it is configured with an `origin` remote pointing to `localhost:2222` for the repo `test/test`.
- Run `git push`
- The server will be protected by key auth, so if the server rejects the SSH connection, run `skyr auth signup --username test --email test@test.com` (`skyr` will be at `target/debug/skyr`)
- The server also requires creating the repo before making the first push. From inside `test-repo/` run `skyr repo create test`; the org comes from the `origin` remote. (Outside a git repo, pass `--org test`.)
- Make any changes you want to the `.scl` files in `test-repo` (they aren't checked into Git)
- Make any commits and pushes you want in `test-repo` too

## Environment Notes

- `cargo` is not available in the current shell session by default.
- `flake.nix` defines a dev shell including `rustup`, `cargo`, `gnumake`, and `protobuf`; use that shell before Rust builds/checks if needed.
- Running tests/builds typically uses `nix develop -c cargo ...`.

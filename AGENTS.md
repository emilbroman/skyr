# Skyr Agent Notes

This file contains guidance for AI agents working on the Skyr codebase. For architecture and crate descriptions, see the [README](README.md) and individual crate READMEs. For end-user documentation, see [docs/](docs/index.md).

### Before Committing

Always run formatting and linting before committing:

```sh
cargo fmt
cargo clippy --all-targets
cargo test
```

Fix any warnings or errors before pushing.

### Conventions and Gotchas

- Keep deployment state transitions coherent across `scs` and `de`.
- When changing schema in `cdb`/`rdb`, update table creation + prepared statements together.
- In `sclc`, parse functions return `Diagnosed<Option<_>>` and report syntax errors via diagnostics instead of `Result<_, ParseError>`.
- In `scl`, the REPL ignores empty lines and uses `Diagnosed` reporting helpers for parse/type diagnostics.
- Whenever the GraphQL server is updated in a way that impacts the schema, regenerate `crates/api/schema.graphql` by running `cargo run -p api -- --write-schema`.
- When writing new RTP plugins, follow the pattern in `plugin_std_random` or `plugin_std_artifact`.
- For ADB operations, configure endpoint/bucket via CLI args or environment variables.
- For LDB logging, use `NamespacePublisher` with deployment QID as namespace.
- The `ids` crate defines the four-level namespace hierarchy (Org → Repo → Environment → Deployment). Use its typed IDs and QIDs rather than raw strings when working with identifiers. Namespace strings (for RDB, LDB, ADB) are QID `.to_string()` values — use environment QIDs for RDB namespaces and deployment QIDs for LDB/ADB namespaces.
- Note: spelling is consistently `supersede/supersession` in schema/API names.
- READMEs and crate-level docs are **internal documentation** aimed at developers working on the codebase. The `docs/` directory contains **external documentation** aimed at end users. When making changes, update the relevant docs to reflect them — but internal-only changes (refactors, internal API changes, implementation details) should **not** be added to external docs.
- When adding new SCL language features (syntax, types, standard library modules/functions, etc.), update the corresponding end-user documentation in `docs/scl/`:
  - `docs/scl/syntax.md` — for new syntax constructs (operators, expressions, statements, keywords)
  - `docs/scl/types.md` — for type system changes (new types, subtyping rules, inference behavior)
  - `docs/scl/stdlib.md` — for new or changed standard library modules and functions
  - `docs/scl/index.md` — if the feature deserves a mention in the "Language Features at a Glance" section

## Running Locally

See the [README](README.md#running-locally) for full service and port listings.

For manual testing:
- Build the CLI with `cargo build -p cli`
- Run `make up`
- Use the local `test-repo/` (gitignored) for Git server tests; it is configured with an `origin` remote pointing to `localhost:2222` for the repo `test/test`.
- Run `git push`
- The server will be protected by key auth, so if the server rejects the SSH connection, run `skyr signup --username test --email test@test.com` (`skyr` will be at `target/debug/skyr`)
- The server also requires creating the repo before making the first push, so if it rejects a push for that reason, run `skyr repo create test/test`
- Make any changes you want to the `.scl` files in `test-repo` (they aren't checked into Git)
- Make any commits and pushes you want in `test-repo` too

## Environment Notes

- `cargo` is not available in the current shell session by default.
- `flake.nix` defines a dev shell including `rustup`, `cargo`, `qemu`, `cdrtools`, and `curl`; use that shell before Rust builds/checks if needed.
- Running tests/builds typically uses `nix develop -c cargo ...`.

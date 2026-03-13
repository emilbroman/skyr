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

### Adding a New RTP Plugin

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
- `flake.nix` defines a dev shell including `rustup`, `cargo`, `gnumake`, and `protobuf`; use that shell before Rust builds/checks if needed.
- Running tests/builds typically uses `nix develop -c cargo ...`.

# Skyr Agent Notes

This file summarizes what is implemented today versus what `docs/index.md` describes as the target system.

## Reality Check (as of current code)

- `docs/index.md` describes a full orchestrator with reconciliation, RTQ/RTE processing, and resource lifecycle operations.
- The repository currently has an early vertical slice:
  - Git-over-SSH config server (`scs`) that can receive and upload Git data.
  - Configuration/deployment database client (`cdb`) with Cassandra schema and typed APIs.
  - Deployment poller/worker loop (`de`) with state transitions and SCL file loading.
  - Skeletons for SCL compiler/runtime data model (`sclc`), resource DB (`rdb`), RTQ (`rtq`), and RTE (`rte`).
- Core reconciliation and resource protocol behavior described in docs is mostly not implemented yet.

## Workspace Structure

- `crates/scs`: SSH Git server and packfile handling, writes Git objects and deployment states to CDB.
- `crates/cdb`: Cassandra-backed API for repositories, objects, deployments, active deployments, supercession links.
- `crates/de`: Daemon that watches active deployments and runs per-deployment reconcile loops.
- `crates/sclc`: SCL front-end + runtime pieces (lexer/parser/AST/type-checker/eval), with a std package and compile pipeline.
- `crates/scl`: CLI/REPL binary for SCL.
- `crates/rdb`: Cassandra schema for resources plus basic ResourceClient CRUD for get/set/delete of inputs/outputs.
- `crates/rte`: Daemon shell only; no RTQ consumption or transition logic.
- `crates/rtq`: Placeholder crate (`src/lib.rs` is effectively empty).

## Implemented Behavior by Area

### SCS (`crates/scs`)

- Implements SSH server with command handling for:
  - `git-receive-pack`
  - `git-upload-pack`
- Accepts all public keys currently (`TODO: authn ...` in code). Do not assume auth is enforced.
- On push:
  - Parses refs update commands.
  - Parses incoming packfiles, resolves deltas, writes Git objects into CDB.
  - Marks new refs as `DESIRED`.
  - Marks replaced refs as `LINGERING` (or `UNDESIRED` on delete).
  - Writes supercession relationship.
- On fetch:
  - Advertises active deployments that are not `UNDESIRED` or `LINGERING` as refs.
  - Streams a generated packfile from stored objects.

### CDB (`crates/cdb`)

- Fully fleshed out compared to other crates:
  - Creates keyspace/tables.
  - Stores and reads Git objects (blob/tree/commit raw/object-level).
  - Stores and queries deployments and active deployments.
  - Supports deployment state updates and supercession lookup.
  - `DeploymentClient` can read files/dirs from commit trees (`read_file`, `read_dir`).
- Note spelling is consistently `supercede/supercession` in schema/API names.

### DE (`crates/de`)

- Daemon starts and polls active deployments every 20s.
- Spawns worker per active deployment; worker loop runs every 5s.
- Handles deployment state branches:
  - `Desired`: compiles SCL (`Main.scl`) and marks superceded deployment `UNDESIRED`.
  - `Undesired`: currently logs teardown and immediately sets state to `DOWN` (resource teardown TODO).
  - `Lingering`: compiles SCL and logs.
  - `Down`: logs and idles.
- No RTQ emissions, no resource graph execution, no health-check restore logic yet.

### SCLC (`crates/sclc`)

- Lexer + PEG parser are implemented for the SCL surface syntax.
- AST/types/value model exists; parser produces AST nodes with spans.
- Type checker and evaluator are implemented (see `checker` and `eval`).
- `Program` supports opening packages, resolving imports, and evaluating a module.
- `compile()` opens `Main.scl`, resolves imports, and type checks, returning `Diagnosed<Program<_>>` with accumulated diagnostics.
- Parser now reports syntax errors as diagnostics (`SyntaxError`), and REPL lines can be empty (`ReplLine { statement: None }`).

### RDB / RTQ / RTE

- `rdb`:
  - Cassandra table definitions exist (`rdb.resources`).
  - `ResourceClient` supports `get`, `set_input`, `set_output`, and `delete`.
- `rtq`: no queue client implementation yet.
- `rte`: daemon loop exists but does no work.

## Gaps Against `docs/index.md`

Not implemented yet (high impact):

- RTQ message model (`CREATE`, `RESTORE`, `ADOPT`, `DESTROY`) and idempotent enqueue/dequeue behavior.
- RTE workers processing transitions and writing outputs/ownership into RDB.
- SCL language semantics: imports, expression evaluation, pending values, dependency tracking.
- DAG execution/reconciliation loop in DE (currently compile-only, no resource operations).
- Health check / drift detection behavior.
- Proper lingering/undesired cleanup based on dependency ownership in RDB.
- Authentication/authorization in SCS.

## Practical Guidance for Future Agents

- Treat `docs/index.md` as target design, not current behavior.
- For bug fixes in existing behavior, start in `scs` and `cdb`; those crates carry most real logic.
- For feature work aligned to docs, expected sequence is:
  1. Define RTQ API and message contract in `crates/rtq`.
  2. Implement RDB resource CRUD in `crates/rdb`.
  3. Extend DE to emit transition intents based on compiled/evaluated config.
  4. Implement RTE consumption/execution path and idempotency handling.
  5. Expand SCLC from file-loader to parser/evaluator with dependency propagation.
- Keep deployment state transitions coherent across `scs` and `de`.
- When changing schema in `cdb`/`rdb`, update table creation + prepared statements together.
- In `sclc`, parse functions return `Diagnosed<Option<_>>` and report syntax errors via diagnostics instead of `Result<_, ParseError>`.
- In `scl`, the REPL ignores empty lines and uses `Diagnosed` reporting helpers for parse/type diagnostics.

## Running Locally (Quick Test)

- Run `podman compose up` to start Cassandra and RabbitMQ.
- Use the local `test-repo/` (gitignored) for Git server tests; it is configured with an `origin` remote pointing to `localhost:2222`.
- Start the `scs` program with `daemon --address 127.0.0.1:2222` so it matches the `test-repo/` remote.
- Start the deployment engine with `cargo run -p de -- daemon` to process deployments.

## Environment Notes

- `cargo` is not available in the current shell session by default.
- `flake.nix` defines a dev shell including `rustup` and `cargo`; use that shell before Rust builds/checks if needed.
- Running tests/builds typically uses `nix develop -c cargo ...`.

# GitHub

The repository is private and is called `emilbroman/skyr`. Use MCP to access it.

Use conventional branch names to associate GH issues. The format is `<issue-number>-<kebab-cased-title>`. This convention can also be used to find the issue of the current branch.

Use MCP to figure out if there is an open PR for the current branch. If I mention "PR" without specifying which one, assume the one attached to the current branch, if any.

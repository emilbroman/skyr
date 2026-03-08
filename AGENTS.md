# Skyr Agent Notes

This file summarizes what is implemented today versus what `docs/index.md` describes as the target system.

## Reality Check (as of current code)

- `docs/index.md` describes a full orchestrator with reconciliation, RTQ/RTE processing, and resource lifecycle operations.
- The repository now has substantial infrastructure:
  - Git-over-SSH config server (`scs`) that can receive and upload Git data.
  - Public API service (`api`) exposing GraphQL auth/signup/me endpoints.
  - User database client (`udb`) backed by Redis for users, tokens, and SSH pubkey fingerprints.
  - Configuration/deployment database client (`cdb`) with Cassandra schema and typed APIs.
  - Deployment poller/worker loop (`de`) with state transitions and SCL file loading.
  - Full SCL compiler/runtime (`sclc`) with lexer/parser/AST/type-checker/eval.
  - Resource database (`rdb`) with inputs/outputs/dependencies/owner tracking.
  - Resource transition queue (`rtq`) with RabbitMQ-backed message passing.
  - Resource transition engine (`rte`) that processes transitions and invokes plugins.
  - Plugin protocol (`rtp`) with gRPC-based communication.
  - Artifact storage (`adb`) with S3-backed artifact database.
  - Log database (`ldb`) with Kafka-backed structured logging.
  - Container orchestrator conduit (`scoc`) with CRI client for containerd.
- Core reconciliation (DE emitting RTQ messages based on compiled config) is still pending.

## Workspace Structure

### Core Services

- `crates/api`: GraphQL API service (signup, bearer-token auth, `me` query, deployment artifacts) backed by `udb`.
- `crates/scs`: SSH Git server and packfile handling, writes Git objects and deployment states to CDB.
- `crates/de`: Daemon that watches active deployments and runs per-deployment reconcile loops.
- `crates/rte`: Resource transition engine daemon processing RTQ messages and invoking plugins.
- `crates/cli`: CLI/REPL binary for SCL (compiled as `skyr`).

### Data Layer

- `crates/udb`: Redis-backed user database client for users, pubkeys, and short-lived bearer tokens.
- `crates/cdb`: Cassandra-backed API for repositories, objects, deployments, active deployments, supercession links.
- `crates/rdb`: Cassandra-backed resource database with inputs, outputs, dependencies, and owner tracking.
- `crates/adb`: S3-backed artifact database with write, read, list, and presigned URL support.
- `crates/ldb`: Kafka-backed log database with publish/consume and severity levels.

### Protocol Layer

- `crates/rtq`: RabbitMQ-backed resource transition queue with Create/Restore/Adopt/Destroy messages.
- `crates/rtp`: gRPC-based resource transition plugin protocol with TCP and Unix socket support.
- `crates/scop`: Skyr Container Orchestrator Protocol with bidirectional gRPC streaming for plugin-conduit communication.

### Compiler & Language

- `crates/sclc`: SCL front-end + runtime pieces (lexer/parser/AST/type-checker/eval), with a std package and compile pipeline.

### Plugins

- `crates/plugin_std_random`: RTP plugin implementing `Std/Random.Int` resource type.
- `crates/plugin_std_artifact`: RTP plugin implementing `Std/Artifact.File` resource type via ADB.
- `crates/plugin_std_container`: Container plugin with Image/Pod/Container resource management (Phases 4-6 complete).

### Container Infrastructure

- `crates/scoc`: Skyr Container Orchestrator Conduit with CRI client for containerd communication.

## Implemented Behavior by Area

### SCS (`crates/scs`)

- Implements SSH server with command handling for:
  - `git-receive-pack`
  - `git-upload-pack`
- Auth validates both:
  - that the SSH username exists in `udb`
  - that the connecting key fingerprint exists in that user's stored pubkey set
- On push:
  - Parses refs update commands.
  - Parses incoming packfiles, resolves deltas, writes Git objects into CDB.
  - Marks new refs as `DESIRED`.
  - Marks replaced refs as `LINGERING` (or `UNDESIRED` on delete).
  - Writes supercession relationship.
- On fetch:
  - Advertises active deployments that are not `UNDESIRED` or `LINGERING` as refs.
  - Streams a generated packfile from stored objects.

### API (`crates/api`)

- Provides GraphQL endpoint and GraphiQL UI.
- Supports:
  - `signup(username, email)` mutation (creates user and issues a bearer token via `udb`)
  - `me` query (requires bearer token, resolves the authenticated user)
  - Deployment artifacts exposure
- Treat this as an early public-API stub; broader domain surface and hardening are still pending.

### UDB (`crates/udb`)

- Redis-backed client with typed APIs for:
  - registering/fetching users
  - setting optional full name
  - issuing/revoking short-lived bearer tokens
  - adding/checking/removing per-user SSH pubkey fingerprints
- Key prefixes:
  - `u:` user hashes
  - `p:` per-user pubkey sets
  - `t:` bearer tokens

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

### RDB (`crates/rdb`)

- Cassandra-backed resource database with full CRUD:
  - `ResourceClient` supports `get`, `set_input`, `set_output`, `set_dependencies`, and `delete`.
  - `NamespaceClient` supports `list_resources` and `list_resources_by_owner`.
  - Resources have `inputs`, `outputs`, `dependencies`, and `owner` fields.
  - Dependencies stored as JSON array of `ResourceId` objects.

### RTQ (`crates/rtq`)

- RabbitMQ-backed message queue with full implementation:
  - Message types: `Create`, `Restore`, `Adopt`, `Destroy`.
  - Each message contains `ResourceRef` (namespace, resource_type, resource_id).
  - `Publisher` for enqueuing messages with consistent-hash sharding.
  - `Consumer` with worker configuration for shard ownership.
  - 32 shards for parallelism, configurable worker index/count.
  - JSON serialization for protocol messages.

### RTE (`crates/rte`)

- Resource transition engine daemon with full message processing:
  - Connects to RTQ as consumer with configurable worker shards.
  - Dials RTP plugins based on `--plugin NAME@TARGET` CLI args.
  - Processes all 4 message types:
    - `Create`: Calls plugin `create_resource`, persists inputs/outputs/dependencies to RDB.
    - `Destroy`: Validates owner, calls plugin `delete_resource`, removes from RDB.
    - `Adopt`: Transfers ownership, optionally calls `update_resource` if inputs differ.
    - `Restore`: Re-applies desired inputs via `update_resource` if they differ.
  - Idempotency: Drops duplicate creates for existing resources, drops deletes for missing/non-owned resources.
  - LDB integration: Logs transition events to deployment log topics.

### RTP (`crates/rtp`)

- gRPC-based resource transition plugin protocol:
  - Protobuf-defined service in `proto/rtp.v1`.
  - `Plugin` trait with `create_resource`, `update_resource`, `delete_resource`, `health` methods.
  - Server: `serve()` function for TCP and Unix socket targets.
  - Client: `dial()` function with capability exchange handshake.
  - `PluginClient` wraps gRPC client with typed methods.
  - Per-connection plugin instances via factory pattern.

### ADB (`crates/adb`)

- S3-backed artifact database:
  - `write()`: Stores artifacts with namespace/name key and media type.
  - `read()`, `read_to_bytes()`, `read_header()`: Retrieve artifacts.
  - `list()`: List all artifacts in a namespace.
  - `presign_read_url()`: Generate time-limited presigned URLs.
  - `private_read_url()`: Generate internal URLs for service-to-service access.
  - Conditional writes (`if-none-match`) for idempotent creates.
  - Configurable endpoint, bucket, credentials, region.

### LDB (`crates/ldb`)

- Kafka-backed log database:
  - `Publisher` and `Consumer` with namespace-scoped topics.
  - `Severity` levels: Info, Warning, Error.
  - `NamespacePublisher` with `info()`, `warn()`, `error()`, `log()` methods.
  - `NamespaceConsumer` with `tail()` streaming method.
  - `TailConfig` for follow mode and start position (from end or beginning).
  - Topic names: `dl-{base64(namespace)}` format.
  - Binary payload format: 8-byte timestamp + 1-byte severity + UTF-8 message.

### SCOC (`crates/scoc`)

- Skyr Container Orchestrator Conduit with CRI client:
  - Connects to containerd via Unix socket (default: `/run/containerd/containerd.sock`).
  - Pod sandbox operations: `run_pod_sandbox`, `stop_pod_sandbox`, `remove_pod_sandbox`.
  - Container operations: `create_container`, `start_container`, `stop_container`, `remove_container`.
  - CLI subcommands for testing: `version`, `pod run/stop/remove`, `container create/start/stop/remove`.
  - Daemon mode: Implements `scop::Conduit` trait, serves SCOP on a TCP port.
  - On startup, registers itself in the node registry (Redis) with its external address.
  - On shutdown, unregisters from the node registry.
  - CLI args: `--node-name`, `--bind`, `--external-address`, `--node-registry-hostname`, `--containerd-socket`.

### SCOP (`crates/scop`)

- Skyr Container Orchestrator Protocol:
  - Bidirectional gRPC streaming between plugin and conduits.
  - gRPC service: `Conduit` with `Session(stream PluginMessage) returns (stream ConduitMessage)`.
  - `Conduit` trait: Implemented by SCOC to handle commands (run/stop/remove pod, create/start/stop/remove container).
  - `ConduitFactory` trait: Creates conduit instances for each incoming connection.
  - `Session`: Handle returned to plugin after connecting, used to send commands.
  - `serve()`: Conduit function to listen for plugin connections (used by SCOC).
  - `dial()`: Plugin function to connect to a conduit (used by container plugin).
  - Target support: TCP (`http://host:port`) and Unix socket (`unix:///path`).
  - Request/response correlation via unique request IDs.

### Plugins

- `plugin_std_random`:
  - Implements `Std/Random.Int` resource type.
  - Inputs: `min`, `max` (integers).
  - Outputs: `result` (random integer in range).

- `plugin_std_artifact`:
  - Implements `Std/Artifact.File` resource type via ADB.
  - Inputs: `namespace`, `name`, `contents`, optional `type` (media type).
  - Outputs: `namespace`, `name`, `media_type`, `url` (private URL).
  - Idempotent creates (treats existing artifacts as success).

- `plugin_std_container`:
  - Full implementation with RTP server and SCOP client (Phases 4-6 complete).
  - Resource types: `Std/Container.Image`, `Std/Container.Pod`, `Std/Container.Pod.Container`.
  - Image resources: Builds images via BuildKit from Git context, pushes to registry.
  - Pod resources: Creates pod sandboxes on worker nodes via SCOP.
  - Container resources: Creates containers within pods, starts them automatically.
  - Connects to SCOC conduits via `scop::dial()` when it needs to run commands.
  - Looks up node addresses from the node registry (Redis).
  - CLI args: `--bind`, `--rtp-bind`, `--node-registry-hostname`, `--cdb-hostnames`, `--buildkit-addr`, `--registry-url`.

## Gaps Against `docs/index.md`

Implemented:

- RTQ message model (`CREATE`, `RESTORE`, `ADOPT`, `DESTROY`) with sharded queues.
- RTE workers processing transitions and writing outputs/ownership into RDB.
- RTP plugin protocol with gRPC communication.
- Artifact storage and logging infrastructure.
- Container orchestrator CRI client (Phase 1).
- SCOP protocol with bidirectional gRPC streaming (Phase 2).
- SCOP conduit server in SCOC with node registry registration (Phase 3).
- Container plugin SCOP client with node registry lookup (Phase 3).
- Container plugin RTP server for resource management (Phase 4).
- Standard library interface for Image/Pod/Container resources (Phase 5).
- BuildKit integration for image builds (Phase 6).

Not implemented yet (high impact):

- DAG execution/reconciliation loop in DE (currently compile-only, no RTQ emissions).
- DE emitting transition intents based on compiled/evaluated SCL config.
- Health check / drift detection behavior.
- Proper lingering/undesired cleanup based on dependency ownership in RDB.
- Fine-grained authorization policy in SCS beyond username+pubkey presence checks.

## Practical Guidance for Future Agents

- Treat `docs/index.md` as target design, not current behavior.
- For bug fixes in existing behavior, start in `scs`, `cdb`, `rte`, or `rtq`; those crates carry most real logic.
- For feature work aligned to docs, the remaining sequence is:
  1. Extend DE to emit RTQ transition intents based on compiled/evaluated SCL config.
  2. Implement dependency propagation in SCLC evaluator.
  3. Add health check / drift detection in RTE.
- Keep deployment state transitions coherent across `scs` and `de`.
- When changing schema in `cdb`/`rdb`, update table creation + prepared statements together.
- In `sclc`, parse functions return `Diagnosed<Option<_>>` and report syntax errors via diagnostics instead of `Result<_, ParseError>`.
- In `scl`, the REPL ignores empty lines and uses `Diagnosed` reporting helpers for parse/type diagnostics.
- Whenever the GraphQL server is updated in a way that impacts the schema, regenerate the `crates/api/schema.graphql` file by running `cargo run -p api -- --write-schema`.
- When writing new RTP plugins, follow the pattern in `plugin_std_random` or `plugin_std_artifact`.
- For ADB operations, configure endpoint/bucket via CLI args or environment variables.
- For LDB logging, use `NamespacePublisher` with deployment ID as namespace.

## Running Locally (Quick Test)

Infrastructure services (via `podman-compose.yml`):
- ScyllaDB (Cassandra-compatible) on port 9042
- RabbitMQ on ports 5672 (AMQP) and 15672 (management UI)
- Redis on port 6379
- Redpanda (Kafka-compatible) on port 9092
- MinIO (S3-compatible) on ports 9000 (API) and 9001 (console)
- OCI Registry on port 5000
- BuildKit on port 1234

Application services:
- `api`: GraphQL API on port 8080
- `scs`: SSH Git server on port 2222
- `de`: Deployment engine
- `rte-{0,1,2}`: Resource transition engine workers (3 instances)
- `plugin-std-random`: Random plugin on port 50051
- `plugin-std-artifact`: Artifact plugin on port 50052
- `plugin-std-container`: Container plugin on port 50053
- `scoc-{1,2,3}`: Container orchestrator conduit nodes

To start everything: `podman compose up` (requires building `skyr:latest` image first).

### VM Mode (QEMU)

For proper container orchestration testing, BuildKit and SCOC nodes can run in QEMU VMs
instead of containers. This avoids the issues with nested containers and privileged mode.

Prerequisites (provided by `nix develop`):
- `qemu` (system emulators)
- `cdrtools` (for `mkisofs`)
- `curl`
- `podman` (for building the SCOC binary via cross-compilation container)

Make targets:
- `make vms`: Start 1 BuildKit VM + 3 SCOC worker VMs (downloads Alpine cloud image on first run)
- `make vms-down`: Stop all VMs
- `make compose-vm`: Full stack with VMs (builds image, starts infra, starts VMs, starts app services)

VM networking:
- BuildKit VM: `tcp://127.0.0.1:1234` (host) → port 1234 in VM
- SCOC-1 VM: `http://127.0.0.1:50061` (host) → port 50054 in VM
- SCOC-2 VM: `http://127.0.0.1:50062` (host) → port 50054 in VM
- SCOC-3 VM: `http://127.0.0.1:50063` (host) → port 50054 in VM
- VMs reach host/podman services via QEMU gateway `10.0.2.2`
- Podman containers reach VMs via `host.containers.internal:<port>`

The `podman-compose.vm.yml` override configures `plugin-std-container` to use the VM-hosted
BuildKit (via `host.containers.internal:1234`) and exposes port 50053 so SCOC VMs can register.

State is stored in `.vm/` (gitignored). Delete `.vm/scoc` to force SCOC binary rebuild.

Configuration (via environment variables):
- `ALPINE_RELEASE`: Alpine version (default: 3.21.4)
- `BUILDKIT_VERSION`: BuildKit version (default: 0.21.1)
- `VM_MEMORY`: RAM per VM (default: 2G)
- `VM_CPUS`: vCPUs per VM (default: 2)

For manual testing:
- Use the local `test-repo/` (gitignored) for Git server tests; it is configured with an `origin` remote pointing to `localhost:2222`.
- Start individual services with `cargo run -p <crate> -- daemon` with appropriate flags.

## Environment Notes

- `cargo` is not available in the current shell session by default.
- `flake.nix` defines a dev shell including `rustup`, `cargo`, `qemu`, `cdrtools`, and `curl`; use that shell before Rust builds/checks if needed.
- Running tests/builds typically uses `nix develop -c cargo ...`.

# GitHub

The repository is private and is called `emilbroman/skyr`. Use MCP to access it.

Use conventional branch names to associate GH issues. The format is `<issue-number>-<kebab-cased-title>`. This convention can also be used to find the issue of the current branch.

Use MCP to figure out if there is an open PR for the current branch. If I mention "PR" without specifying which one, assume the one attached to the current branch, if any.

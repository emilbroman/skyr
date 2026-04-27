# Skyr

Skyr is a Git-native infrastructure orchestrator. You define your infrastructure in code using the Skyr Configuration Language (SCL), push to Skyr's Git server, and watch your resources come to life.

## Core Concepts

**Git as the interface.** Skyr hosts Git repositories organized into *organizations* and *repositories*. Each branch or tag becomes an *environment*, and each commit pushed to an environment creates a *deployment*. Skyr reconciles your infrastructure to match the desired state.

**Declarative configuration.** You describe *what* you want, not *how* to create it. Skyr handles the lifecycle: creating, updating, and destroying resources as your configuration evolves.

**Resource DAG.** Resources can depend on other resources. Skyr evaluates your configuration and builds a directed acyclic graph (DAG) of dependencies, ensuring resources are created in the correct order.

**Self-healing.** Skyr continuously monitors your resources and restores them if they drift from the desired state.

See [Deployments](deployments.md) to learn how deployments roll out, supersede each other, and clean up resources. See [Status and Incidents](status.md) for how Skyr surfaces health and notifies you when things go wrong. See [Cross-Repo Imports](cross-repo-imports.md) for sharing modules and reading remote state across repositories.

## SCL: Skyr Configuration Language

SCL is a statically-typed functional language designed for infrastructure configuration. Here's what it looks like:

```scl
import Std/Container

let image = Container.Image({
    name: "my-app",
    context: ".",
    containerfile: "Containerfile",
})

let pod = Container.Pod({
    name: "my-app",
    containers: [{ image: image.fullname }],
})
```

This configuration:
1. Builds a container image from the repository's root directory
2. Creates a pod running a container with the built image

### Learn SCL

- [Language Overview](scl/index.md) — Introduction and quick start
- [Syntax Reference](scl/syntax.md) — Expressions, statements, and operators
- [Type System](scl/types.md) — Types, inference, and annotations
- [Standard Library](/~docs/scl/stdlib-ref/) — Built-in modules and resources

## How Resources Work

When you call a resource function like `Container.Image(...)`, Skyr:

1. **Derives a unique ID** from the resource type and inputs
2. **Checks if it exists** — if so, returns the existing outputs
3. **Creates it if needed** — queues a creation task and tracks dependencies
4. **Tracks ownership** — associates the resource with your deployment

When your configuration changes:
- New resources are created
- Changed resources are updated
- Removed resources are destroyed
- Unchanged resources are preserved

When a deployment supersedes another (e.g., pushing a new commit to the same environment), Skyr transfers ownership of shared resources to the new deployment and cleans up orphaned resources. All resources within an environment share a namespace, so adoption between deployments is seamless.

## Getting Started

### Entry Point

Every Skyr repository needs a `Main.scl` file at the root. This is the entry point for your configuration.

### Example: A Complete Application

```scl
import Std/Container
import Std/Artifact
import Std/Encoding

// Build the application image
let image = Container.Image({
    name: "hello-world",
    context: ".",
    containerfile: "Containerfile",
})

// Create a pod running the application
let pod = Container.Pod({
    name: "hello-world",
    containers: [{ image: image.fullname }],
})

// Expose deployment info as an artifact
Artifact.File({
    name: "deployment-info.json",
    mediaType: "application/json",
    contents: Encoding.toJson({
        pod: pod.name,
        image: image.fullname,
    }),
})
```

With a `Containerfile` like:

```dockerfile
FROM alpine:latest
CMD ["echo", "Hello from Skyr!"]
```

Push to Skyr:

```bash
git add .
git commit -m "Initial deployment"
git push skyr main
```

## CLI Reference

The `skyr` CLI helps you work with SCL locally and interact with Skyr servers.

```bash
# Local
skyr repl                                   # Interactive SCL REPL
skyr run                                    # Evaluate Main.scl in the current directory
skyr run --root ./app                       # Evaluate Main.scl in a specific directory
skyr fmt Main.scl                           # Format an SCL file

# Auth
skyr auth signup --username alice --email alice@example.com
skyr auth signin --username alice
skyr auth whoami
skyr auth signout

# Tier-2 commands. From a working tree on the `main` branch of `alice/my-app`,
# org/repo come from the `skyr` remote (or `origin` as a fallback) and env from the current branch.
skyr repo list
skyr repo create my-app                     # creates alice/my-app from current org
skyr deployments list                       # alice/my-app
skyr deployments logs --follow              # alice/my-app, all envs
skyr resources list                         # alice/my-app, env=main
skyr resources list --env staging           # alice/my-app, env=staging
skyr resources logs alice/my-app::main::Std/Random.Int:dice
skyr resources delete Std/Random.Int:dice   # alice/my-app, env=main

# Direct GraphQL access
skyr api query  '{ me { username } }'
skyr api mut    '{ createOrganization(name: $name) { name } }' --arg=name=acme
```

Global flags can be set via env vars: `SKYR_API_URL`, `SKYR_ORG`, `SKYR_REPO`,
`SKYR_ENV`. Use `--format json` with any command to get machine-readable
output. The org/repo/env defaults can be overridden per-invocation with
`--org`, `--repo`, `--env`.

### `api query` / `api mut`

The body you pass is just a GraphQL selection set; the CLI wraps it into a
named operation, generating the variable signature from your `--arg` flags.

A spec is `<name>(:<type>)?(=<value>)?`:

- `--arg=enabled`                 → `$enabled: Boolean!`, value `true`
- `--arg=name=alice`              → `$name: String!`, value `"alice"`
- `--arg=count=42`                → `$count: Int!`, value `42`
- `--arg=filter='{"k":1}'`        → `$filter: JSON!`, value `{"k":1}`
- `--arg=input:UserInput='{...}'` → `$input: UserInput!`, value parsed as JSON
- `--arg=tag:Tag`                 → `$tag: Tag!`, value `true` (sentinel)
- `--arg=name:String=42`          → `$name: String!`, value `"42"` (forced)
- `--arg=parent:User=null`        → `$parent: User`, value `null`

Resolution rules:

1. If the value is omitted, value = `true`.
2. With explicit type `String` (or `String!`), the raw value is taken
   literally — never JSON-parsed.
3. Otherwise the value is JSON-parsed; if no type was given and parsing
   fails, it is taken as a string.
4. If no type was given, derive: `bool`→`Boolean`, integer JSON number→`Int`,
   non-integer JSON number→`Float`, JSON string→`String`, object/array→`JSON`.
   `null` requires an explicit type.
5. The CLI appends `!` to make the type non-null, unless the resolved value
   is JSON `null`.

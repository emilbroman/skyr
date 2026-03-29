# Skyr

Skyr is a Git-native infrastructure orchestrator. You define your infrastructure in code using the Skyr Configuration Language (SCL), push to Skyr's Git server, and watch your resources come to life.

## Core Concepts

**Git as the interface.** Skyr hosts Git repositories organized into *organizations* and *repositories*. Each branch or tag becomes an *environment*, and each commit pushed to an environment creates a *deployment*. Skyr reconciles your infrastructure to match the desired state.

**Declarative configuration.** You describe *what* you want, not *how* to create it. Skyr handles the lifecycle: creating, updating, and destroying resources as your configuration evolves.

**Resource DAG.** Resources can depend on other resources. Skyr evaluates your configuration and builds a directed acyclic graph (DAG) of dependencies, ensuring resources are created in the correct order.

**Self-healing.** Skyr continuously monitors your resources and restores them if they drift from the desired state.

See [Deployments](deployments.md) to learn how deployments roll out, supersede each other, and clean up resources.

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
skyr repl                              # Interactive SCL REPL
skyr run                               # Evaluate Main.scl in the current directory
skyr run --root ./app                  # Evaluate Main.scl in a specific directory
skyr fmt Main.scl                      # Format an SCL file
skyr signup --username alice --email alice@example.com  # Create a new account
skyr signin --username alice           # Sign in to Skyr
skyr whoami                            # Show current user
skyr repo list                         # List repositories
skyr repo create alice/my-app          # Create a repository
skyr deployments list alice/my-app     # List deployments
skyr deployments logs alice/my-app     # Stream deployment logs
skyr resources list alice/my-app       # List resources
skyr resources logs alice/my-app::main::Std/Random.Int:dice  # Stream resource logs
```

Use `--format json` with any command to get machine-readable output. Use `--api-url` to point at a different Skyr server.

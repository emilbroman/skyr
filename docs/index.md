# Skyr

Skyr is a Git-native infrastructure orchestrator. You define your infrastructure in code using the Skyr Configuration Language (SCL), push to Skyr's Git server, and watch your resources come to life.

## Core Concepts

**Git as the interface.** Skyr hosts Git repositories. When you push a commit, Skyr creates a deployment and begins reconciling your infrastructure to match the desired state.

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

let pod = Container.Pod({ name: "my-app" })

pod.Container({
    name: "app",
    image: image.fullname,
})
```

This configuration:
1. Builds a container image from the repository's root directory
2. Creates a pod (a group of containers that share resources)
3. Runs a container using the built image

### Learn SCL

- [Language Overview](scl/index.md) — Introduction and quick start
- [Syntax Reference](scl/syntax.md) — Expressions, statements, and operators
- [Type System](scl/types.md) — Types, inference, and annotations
- [Standard Library](scl/stdlib.md) — Built-in modules and resources

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

When a deployment supersedes another (e.g., pushing a new commit to the same branch), Skyr transfers ownership of shared resources to the new deployment and cleans up orphaned resources.

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

// Create a pod to run the application
let pod = Container.Pod({ name: "hello-world" })

// Run the container
let container = pod.Container({
    name: "app",
    image: image.fullname,
})

// Expose deployment info as an artifact
Artifact.File({
    name: "deployment-info.json",
    type: "application/json",
    contents: Encoding.toJson({
        pod: pod.name,
        container: container.name,
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
skyr repl                    # Interactive SCL REPL
skyr run                     # Evaluate Main.scl in the current directory
skyr run --root ./app        # Evaluate Main.scl in a specific directory
skyr signup                  # Create a new account
skyr signin                  # Sign in to Skyr
skyr whoami                  # Show current user
skyr repo                    # Manage repositories
skyr deployments             # View deployment status
```

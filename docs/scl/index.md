# SCL: Skyr Configuration Language

SCL is a statically-typed functional language designed for infrastructure configuration. You describe *what* resources you want, and Skyr handles the *how*.

## Quick Example

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
1. Builds a container image from your repository
2. Creates a pod with a container using the built image

Skyr automatically handles the dependency order — the pod waits for the image to be ready before starting.

## Getting Started

Every Skyr repository needs a `Main.scl` file at the root. This is the entry point for your configuration.

```scl
// Main.scl
import Std/Artifact

Artifact.File({
    name: "hello.txt",
    contents: "Hello, world!",
})
```

Push to Skyr and your artifact is created:

```bash
git add Main.scl
git commit -m "Initial configuration"
git push skyr main
```

## Documentation

- [Syntax Reference](syntax.md) — Expressions, statements, and operators
- [Type System](types.md) — Types, type inference, and type annotations
- [Standard Library](/~docs/scl/stdlib-ref/) — Built-in modules and resources

## Language Features at a Glance

### Values and Types

```scl
let count = 42                             // Int
let ratio = 3.14                           // Float
let name = "my-app"                        // Str
let enabled = true                         // Bool
let nothing = nil                          // Never?
let items = [1, 2, 3]                      // [Int]
let config = { port: 8080, debug: false }  // { port: Int, debug: Bool }
let lookup = #{ "key": "value" }           // #{ Str: Str }
```

### String Interpolation

Embed expressions directly in strings:

```scl
let greeting = "Hello, {name}!"
let info = "Running {count} instances on port {config.port}"
```

### Functions

First-class functions with type inference:

```scl
let double = fn(x: Int) x * 2
let result = double(21)  // 42
```

Generic functions work with any type:

```scl
let identity = fn<T>(x: T) x
let first = fn<T <: { name: Str }>(item: T) item.name
```

### Exception Handling

Define exceptions and handle them with `try`/`catch`:

```scl
import Std/Option

let value = try Option.unwrap(maybeValue)
    catch Option.UnexpectedNil: "fallback"
```

### List Comprehensions

Transform and filter collections:

```scl
let doubled = [for (x in items) x * 2]
let positive = [for (x in items) if (x > 0) x]
```

### Conditionals

```scl
let status = if (enabled) "on" else "off"
```

### Type Declarations

Name and export types for reuse across modules:

```scl
export type Config { host: Str, port: Int }

import MyLib
let cfg: MyLib.Config = { host: "localhost", port: 8080 }
```

### Modules

Organize code with imports. Standard library modules use the `Std/` prefix:

```scl
import Std/Encoding
import Std/Artifact

Artifact.File({
    name: "config.json",
    contents: Encoding.toJson({ version: 1 }),
})
```

You can also import your own `.scl` files using your repository's qualified name:

```scl
import alice/my-repo/Config
let port = Config.defaultPort
```

See [Syntax Reference — Import](syntax.md#import) for details.

## Resources

Resources are the building blocks of your infrastructure. When you call a resource function like `Container.Image(...)`, Skyr:

1. **Derives a unique ID** from the resource type and inputs
2. **Checks if it exists** — if so, returns the existing outputs
3. **Creates it if needed** — queues a creation task
4. **Tracks dependencies** — records which resources depend on which

Resources return records containing their outputs. Use these outputs in other resources to establish dependencies:

```scl
let image = Container.Image({
    name: "app",
    context: ".",
    containerfile: "Containerfile",
})

// image.fullname creates a dependency on the image
let pod = Container.Pod({
    name: "app",
    containers: [{ image: image.fullname }],  // Uses the built image
})
```

When your configuration changes across deployments:
- New resources are created
- Changed resources are updated
- Removed resources are destroyed
- Unchanged resources are preserved

See [Deployments](../deployments.md) for details on how resources transition between deployments.

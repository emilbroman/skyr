# Standard Library

The SCL standard library provides modules for common tasks: building containers, creating artifacts, encoding data, and more.

## Std/Container

Build and run containers on the Skyr cluster.

### Image

Build a container image from a directory in your repository:

```scl
import Std/Container

let image = Container.Image({
    name: "my-app",
    context: ".",
    containerfile: "Containerfile",
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Image name (without registry prefix) |
| `context` | `Str` | Path to build context directory, relative to repo root |
| `containerfile` | `Str` | Path to Containerfile/Dockerfile, relative to context |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `fullname` | `Str` | Full image reference including registry and digest (e.g., `registry:5000/my-app@sha256:...`) |
| `digest` | `Str` | Image digest (e.g., `sha256:...`) |

The image is built using BuildKit and pushed to Skyr's container registry. The resource ID is derived from the Git tree hash of the context directory, so rebuilds only happen when source files change.

### Pod

Create a pod sandbox on a worker node:

```scl
let pod = Container.Pod({ name: "my-pod" })
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Pod name |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `podId` | `Str` | Unique pod identifier |
| `node` | `Str` | Worker node hosting this pod |
| `name` | `Str` | Pod name |
| `namespace` | `Str` | Namespace (deployment ID) |
| `Container` | `fn({...}) {...}` | Function to create containers in this pod |

A pod is a group of containers that share resources. Use the `Container` method to add containers.

### Container

Create a container within a pod. Accessed via `pod.Container`:

```scl
let container = pod.Container({
    name: "app",
    image: image.fullname,
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Container name |
| `image` | `Str` | Full image reference (use `image.fullname`) |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `containerId` | `Str` | Unique container identifier |
| `name` | `Str` | Container name |
| `image` | `Str` | Image used |

### Complete Example

```scl
import Std/Container

// Build the image
let image = Container.Image({
    name: "hello-world",
    context: ".",
    containerfile: "Containerfile",
})

// Create a pod
let pod = Container.Pod({ name: "hello-world" })

// Run a container in the pod
pod.Container({
    name: "app",
    image: image.fullname,
})
```

## Std/Artifact

Store files as artifacts. Artifacts persist even after deployments are torn down.

### File

Create a downloadable file artifact:

```scl
import Std/Artifact

let readme = Artifact.File({
    name: "readme.txt",
    contents: "Hello, world!",
    type: "text/plain",
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Artifact name (unique within deployment) |
| `contents` | `Str` | File contents |
| `type` | `Str?` | Media type (optional, defaults to `application/octet-stream`) |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `url` | `Str` | URL to download the artifact |

Artifacts are stored in Skyr's object storage. The URL is a presigned link that allows downloading the file.

### Example: Configuration Export

```scl
import Std/Artifact
import Std/Encoding

let config = {
    version: 1,
    features: ["logging", "metrics"],
}

Artifact.File({
    name: "config.json",
    type: "application/json",
    contents: Encoding.toJson(config),
})
```

## Std/Encoding

Serialize and deserialize data.

### toJson

Convert any value to a JSON string:

```scl
import Std/Encoding

let json = Encoding.toJson({ key: "value", count: 42 })
// "{\"key\":\"value\",\"count\":42}"
```

**Type:** `fn(Any) Str`

Value mappings:

| SCL Type | JSON Type |
|----------|-----------|
| `Int` | `number` |
| `Float` | `number` |
| `Str` | `string` |
| `Bool` | `true` / `false` |
| `Never?` (`nil`) | `null` |
| `List` | `array` |
| `Record` | `object` |
| `Dict` | `object` (non-string keys are stringified) |

Functions cannot be serialized and will cause a runtime error.

### fromJson

Parse a JSON string into a value:

```scl
let data = Encoding.fromJson("{\"key\":\"value\"}")
// { key: "value" }
```

**Type:** `fn(Str) Any`

Returns `Any`, so you may need to access fields dynamically:

```scl
let config = Encoding.fromJson(jsonString)
let port = config.port   // Dynamically typed
```

Value mappings:

| JSON Type | SCL Type |
|-----------|----------|
| `number` | `Float` |
| `string` | `Str` |
| `true` / `false` | `Bool` |
| `null` | `Never?` (`nil`) |
| `array` | `List` |
| `object` | `Dict` (with `Str` keys) |

Note: JSON numbers become `Float`, not `Int`.

## Std/Random

Generate random values. Useful for testing and development.

### Int

Generate a random integer in a range:

```scl
import Std/Random

let roll = Random.Int({
    name: "dice",
    min: 1,
    max: 6,
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Resource identifier (must be unique) |
| `min` | `Int` | Minimum value (inclusive) |
| `max` | `Int` | Maximum value (inclusive) |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `min` | `Int` | Input min value |
| `max` | `Int` | Input max value |
| `result` | `Int` | The generated random integer |

The random value is generated once when the resource is created and remains stable across subsequent evaluations.

## Std/Num

Numeric utilities.

### toHex

Convert an integer to a hexadecimal string:

```scl
import Std/Num

let hex = Num.toHex(255)   // "ff"
let big = Num.toHex(65535) // "ffff"
```

**Type:** `fn(Int) Str`

Returns lowercase hexadecimal without prefix.

## Using Multiple Modules

Combine modules for more complex configurations:

```scl
import Std/Container
import Std/Artifact
import Std/Encoding

// Build and run an application
let image = Container.Image({
    name: "api",
    context: "./api",
    containerfile: "Containerfile",
})

let pod = Container.Pod({ name: "api" })

let container = pod.Container({
    name: "api",
    image: image.fullname,
})

// Export deployment information
Artifact.File({
    name: "deployment.json",
    type: "application/json",
    contents: Encoding.toJson({
        image: image.fullname,
        pod: pod.name,
        node: pod.node,
        container: container.name,
    }),
})
```

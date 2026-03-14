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

To allow a pod to reach another pod's port, pass port resources in the `allow` list:

```scl
let dbPort = dbPod.Port({ port: 5432, protocol: "tcp" })
let apiPod = Container.Pod({ name: "api", allow: [dbPort] })
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Pod name |
| `allow` | `[{ address: Str, port: Int, protocol: Str }]?` | Port resources this pod can reach (optional) |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `podId` | `Str` | Unique pod identifier |
| `node` | `Str` | Worker node hosting this pod |
| `name` | `Str` | Pod name |
| `namespace` | `Str` | Namespace (deployment ID) |
| `address` | `Str` | Pod IP address |
| `Container` | `fn({...}) {...}` | Function to create containers in this pod |
| `Port` | `fn({...}) {...}` | Function to expose ports on this pod |

A pod is a group of containers that share resources. Use the `Container` method to add containers and `Port` to expose network ports.

By default, pods are network-isolated: all ingress is denied, and egress to other cluster pods is blocked. Pods can always reach the internet. To allow a pod to communicate with another pod, use `Pod.Port` to open an ingress port, then pass that port resource in the `allow` list of the pod that needs to connect.

### Port

Expose a port on a pod's firewall. Accessed via `pod.Port`:

```scl
let httpPort = pod.Port({ port: 8080, protocol: "tcp" })
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `port` | `Int` | Port number to open |
| `protocol` | `Str` | Protocol: `"tcp"` (default) or `"udp"` |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `address` | `Str` | The pod's IP address |
| `port` | `Int` | The opened port number |
| `protocol` | `Str` | The protocol |

Port resources act as access tokens. Pass them to another pod's `allow` list to grant that pod egress access to this port.

### Host

Create a virtual load balancer with a cluster-internal DNS name:

```scl
let apiHost = Container.Host({ name: "api" })
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Host name (becomes `{name}.internal` for DNS) |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `hostname` | `Str` | Full DNS hostname (e.g., `api.internal`) |
| `Port` | `fn({...}) {...}` | Function to create load-balanced ports on this host |

A Host is a virtual network appliance — it doesn't run any containers. It acts as a DNS entry and load balancer. Use `host.Port` to create load-balanced ports that route to backend pod ports.

### Host.Port

Create a load-balanced port on a Host. Accessed via `host.Port`:

```scl
let apiHostPort = apiHost.Port({
    port: 80,
    backends: [apiPort1, apiPort2, apiPort3],
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `port` | `Int` | Port number to expose on the Host VIP |
| `backends` | `[{ address: Str, port: Int, protocol: Str }]` | Backend ports to load-balance across (Pod.Port or Host.Port) |
| `protocol` | `Str?` | Protocol: `"tcp"` (default) or `"udp"` |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `hostname` | `Str` | The Host's DNS hostname |
| `address` | `Str` | The Host's VIP address |
| `port` | `Int` | The exposed port number |
| `protocol` | `Str` | The protocol |

Host.Port resources can be passed to a pod's `allow` list just like Pod.Port resources. Traffic is load-balanced across backends using round-robin. Backends can be Pod.Port or Host.Port resources, enabling complex routing topologies such as internal API gateways:

```scl
// Chain Host.Ports to build a gateway that routes through backend services
let gateway = Container.Host({ name: "gateway" })
let gatewayPort = gateway.Port({
    port: 80,
    backends: [userServicePort, orderServicePort],
})
```

When a Host.Port is used as a backend, traffic is forwarded through its own load-balancing rules to the ultimate pod backends.

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
| `command` | `[Str]?` | Entrypoint command (optional) |
| `args` | `[Str]?` | Command arguments (optional) |
| `envs` | `#{ Str: Str }?` | Environment variables (optional) |

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

// Create a pod with a container
let pod = Container.Pod({ name: "hello-world" })
pod.Container({
    name: "app",
    image: image.fullname,
})
let httpPort = pod.Port({ port: 8080, protocol: "tcp" })

// Another pod that can access the first pod's HTTP port
let clientPod = Container.Pod({ name: "client", allow: [httpPort] })
clientPod.Container({
    name: "curl",
    image: "curlimages/curl",
})
```

### Networking Example

```scl
import Std/Container

// Database tier
let dbPod = Container.Pod({ name: "postgres" })
dbPod.Container({
    name: "postgres",
    image: "postgres:16",
    envs: { POSTGRES_PASSWORD: "secret" },
})
let dbPort = dbPod.Port({ port: 5432 })

// API tier with access to the database
let apiPod = Container.Pod({ name: "api", allow: [dbPort] })
apiPod.Container({
    name: "api",
    image: image.fullname,
    envs: { DATABASE_URL: "postgres://postgres:secret@{dbPort.address}:5432/app" },
})
let apiPort = apiPod.Port({ port: 8080 })

// Load-balanced API host
let apiHost = Container.Host({ name: "api" })
let apiHostPort = apiHost.Port({ port: 80, backends: [apiPort] })

// Frontend pod that accesses the API via DNS
let frontendPod = Container.Pod({ name: "frontend", allow: [apiHostPort] })
frontendPod.Container({
    name: "nginx",
    image: "nginx:latest",
    envs: { API_URL: "http://{apiHostPort.hostname}:{apiHostPort.port}" },
})
```

In this example:
- The database is only reachable by the API pod (via `dbPort`)
- The API is load-balanced behind `api.internal:80`
- The frontend reaches the API via the Host DNS name
- No pod can reach the database except the API tier

## Std/Artifact

Store files as artifacts. Artifacts persist even after deployments are torn down.

### File

Create a downloadable file artifact:

```scl
import Std/Artifact

let readme = Artifact.File({
    name: "readme.txt",
    contents: "Hello, world!",
    mediaType: "text/plain",
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Artifact name (unique within deployment) |
| `contents` | `Str` | File contents |
| `mediaType` | `Str?` | Media type (optional, defaults to `application/octet-stream`) |

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
    mediaType: "application/json",
    contents: Encoding.toJson(config),
})
```

## Std/Crypto

Generate and manage cryptographic key pairs. Keys persist across deployments.

All three key types share the same output shape `{ pem: Str, publicKeyPem: Str }`, so they are interchangeable via structural subtyping.

### ED25519PrivateKey

Generate an Ed25519 key pair:

```scl
import Std/Crypto

let key = Crypto.ED25519PrivateKey({ name: "deploy-key" })
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Key identifier (unique within environment) |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `pem` | `Str` | Private key in PKCS#8 PEM format |
| `publicKeyPem` | `Str` | Public key in SPKI PEM format |

### ECDSAPrivateKey

Generate an ECDSA key pair on a specified curve:

```scl
let key = Crypto.ECDSAPrivateKey({
    name: "signing-key",
    curve: "P-384",
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Key identifier (unique within environment) |
| `curve` | `Str?` | Elliptic curve: `"P-256"` (default), `"P-384"`, or `"P-521"` |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `pem` | `Str` | Private key in PKCS#8 PEM format |
| `publicKeyPem` | `Str` | Public key in SPKI PEM format |

### RSAPrivateKey

Generate an RSA key pair:

```scl
let key = Crypto.RSAPrivateKey({
    name: "tls-key",
    size: 4096,
})
```

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `name` | `Str` | Key identifier (unique within environment) |
| `size` | `Int?` | Key size in bits (default `2048`, minimum `2048`) |

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `pem` | `Str` | Private key in PKCS#8 PEM format |
| `publicKeyPem` | `Str` | Public key in SPKI PEM format |

### CertificationRequest

Generate a PKCS#10 Certificate Signing Request (CSR) from an existing private key:

```scl
let key = Crypto.ECDSAPrivateKey({ name: "tls-key", curve: "P-256" })

let csr = Crypto.CertificationRequest({
    privateKeyPem: key.pem,
    subject: {
        commonName: "example.com",
        organization: "My Corp",
        country: "US",
    },
    subjectAlternativeNames: ["example.com", "*.example.com", "192.168.1.1"],
    keyUsage: ["digitalSignature", "keyEncipherment"],
    extendedKeyUsage: ["serverAuth", "clientAuth"],
})
```

The resource is identified by a hash of its inputs rather than an explicit name, so changing any input produces a new CSR.

**Inputs:**

| Field | Type | Description |
|-------|------|-------------|
| `privateKeyPem` | `Str` | Private key PEM (from any `*PrivateKey` resource) |
| `subject.commonName` | `Str` | Common Name (CN) |
| `subject.organization` | `Str?` | Organization (O) |
| `subject.organizationalUnit` | `Str?` | Organizational Unit (OU) |
| `subject.country` | `Str?` | Country (C) |
| `subject.state` | `Str?` | State or Province (ST) |
| `subject.locality` | `Str?` | Locality (L) |
| `subjectAlternativeNames` | `[Str]?` | SANs — auto-detected as DNS, IP, or email |
| `keyUsage` | `[Str]?` | Key usage flags (e.g. `"digitalSignature"`, `"keyEncipherment"`) |
| `extendedKeyUsage` | `[Str]?` | Extended key usage OIDs (e.g. `"serverAuth"`, `"clientAuth"`) |

**Supported `keyUsage` values:** `digitalSignature`, `nonRepudiation`, `contentCommitment`, `keyEncipherment`, `dataEncipherment`, `keyAgreement`, `keyCertSign`, `cRLSign`, `encipherOnly`, `decipherOnly`

**Supported `extendedKeyUsage` values:** `serverAuth`, `clientAuth`, `codeSigning`, `emailProtection`, `timeStamping`, `ocspSigning`

**Outputs:**

| Field | Type | Description |
|-------|------|-------------|
| `pem` | `Str` | Signed CSR in PEM format |

> **Note:** P-521 keys are not currently supported for certification requests.

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

The random value is generated when the resource is created and regenerated on updates (e.g., when `min` or `max` change).

## Std/Option

Utilities for working with optional (`T?`) values.

### isNone

Check if a value is `nil`:

```scl
import Std/Option

Option.isNone<Int>(nil)    // true
Option.isNone<Int>(42)     // false
```

**Type:** `fn<T>(value: T?) Bool`

### isSome

Check if a value is not `nil`:

```scl
Option.isSome<Int>(42)     // true
Option.isSome<Int>(nil)    // false
```

**Type:** `fn<T>(value: T?) Bool`

### unwrap

Extract the value from an optional, or raise an exception if `nil`:

```scl
Option.unwrap<Int>(42)     // 42
Option.unwrap<Int>(nil)    // raises UnexpectedNil
```

**Type:** `fn<T>(value: T?) T`

Raises the `Option.UnexpectedNil` exception if the value is `nil`. Use `try`/`catch` to handle:

```scl
let result = try Option.unwrap<Str>(maybeValue)
    catch Option.UnexpectedNil: "fallback"
```

### default

Return the value if present, or a fallback if `nil`:

```scl
Option.default<Int>(42, 0)     // 42
Option.default<Int>(nil, 0)    // 0
```

**Type:** `fn<T>(value: T?, fallback: T) T`

### UnexpectedNil

An exception type, raised by `unwrap` when the value is `nil`.

## Std/List

List utilities.

### range

Generate a list of integers from 0 up to (but not including) `n`:

```scl
import Std/List

let indices = List.range(5)   // [0, 1, 2, 3, 4]
```

**Type:** `fn(Int) [Int]`

Returns a list containing every integer in the half-open range `[0, n)`. Returns an empty list when `n` is zero. Raises an error if `n` is negative.

### map

Apply a function to each element:

```scl
List.map<Int, Int>([1, 2, 3], fn(x: Int) x * 2)   // [2, 4, 6]
```

**Type:** `fn<T, U>(list: [T], transform: fn(T) U) [U]`

### filter

Keep only elements that satisfy a predicate:

```scl
List.filter<Int>([1, 2, 3, 4], fn(x: Int) x > 2)   // [3, 4]
```

**Type:** `fn<T>(list: [T], predicate: fn(T) Bool) [T]`

### append

Add an element to the end of a list:

```scl
List.append<Int>([1, 2], 3)   // [1, 2, 3]
```

**Type:** `fn<T>(list: [T], newItem: T) [T]`

### concat

Flatten a list of lists into a single list:

```scl
List.concat<Int>([[1, 2], [3, 4]])   // [1, 2, 3, 4]
```

**Type:** `fn<T>(lists: [[T]]) [T]`

### flatMap

Map each element to a list, then flatten:

```scl
List.flatMap<Int, Int>([1, 2, 3], fn(x: Int) [x, x * 10])   // [1, 10, 2, 20, 3, 30]
```

**Type:** `fn<T, U>(list: [T], transform: fn(T) [U]) [U]`

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

## Std/Time

Time utilities.

### Instant

A point in time, represented as milliseconds since the Unix epoch:

```scl
import Std/Time

let t: Time.Instant = { epochMillis: 1700000000000 }
```

| Field | Type | Description |
|-------|------|-------------|
| `epochMillis` | `Int` | Milliseconds since 1970-01-01T00:00:00Z |

### Duration

A time span with optional month and millisecond components:

```scl
let oneHour: Time.Duration = { milliseconds: 3600000 }
let quarterly: Time.Duration = { months: 3 }
let mixed: Time.Duration = { months: 1, milliseconds: 1 }
```

| Field | Type | Description |
|-------|------|-------------|
| `milliseconds` | `Int?` | Millisecond component (optional) |
| `months` | `Int?` | Calendar month component (optional) |

Both fields are optional. The month component uses calendar-month arithmetic (adding 1 month to Jan 31 gives Feb 28/29), while the millisecond component is exact.

### Clock

Create a volatile clock resource that produces a time-window `Instant`. The clock truncates the current time to the closest past boundary of the given duration, aligned with the Unix epoch:

```scl
import Std/Time

let hourly = Time.Clock({ milliseconds: 3600000 })
let monthly = Time.Clock({ months: 1 })
let custom = Time.Clock({ months: 1, milliseconds: 1 })
```

**Input:** `Duration` — the window size.

**Output:** `Instant` — the start of the current window.

The resource is volatile: the deployment engine periodically re-checks it. When the clock crosses a window boundary the output changes, triggering dependent resources to update.

**Boundary calculation:**

1. Find the largest epoch-aligned month boundary at or before the current time.
2. From that month boundary, find the largest millisecond-aligned boundary at or before the current time.

For example, with `{ months: 1, milliseconds: 1 }`:
- Window 1 starts at `1970-01-01T00:00:00.000Z`
- Window 2 starts at `1970-02-01T00:00:00.001Z`
- Window 3 starts at `1970-03-01T00:00:00.002Z`

### toISO

Format an `Instant` as a UTC ISO 8601 string with second precision:

```scl
let iso = Time.toISO({ epochMillis: 1700000000000 })
// "2023-11-14T22:13:20Z"
```

**Type:** `fn(Instant) Str`

Returns a string in the format `YYYY-MM-DDTHH:MM:SSZ`.

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
    mediaType: "application/json",
    contents: Encoding.toJson({
        image: image.fullname,
        pod: pod.name,
        node: pod.node,
        container: container.name,
    }),
})
```

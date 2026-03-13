# Skyr Resource Transition Plugin Protocol (RTP)

RTP defines the gRPC protocol used by the [RTE](../rte/) to communicate with resource plugins.

## Role in the Architecture

RTP is the boundary between Skyr's core and its extensible plugin system. Any resource type is implemented as an RTP plugin that responds to create, update, delete, and health check requests.

```
RTE → RTP (gRPC) → Plugin
```

## Protocol

The protocol is defined in `proto/rtp.v1` (protobuf) and exposes:

### Plugin Trait

Plugins implement the `Plugin` trait:

| Method | Description |
|--------|-------------|
| `create_resource` | Create a new resource from inputs, return outputs |
| `update_resource` | Update an existing resource with new inputs |
| `delete_resource` | Delete a resource (optional, default no-op) |
| `health` | Health check for a resource (optional, default no-op) |

### Server

`serve()` starts an RTP server on a TCP or Unix socket target. Per-connection plugin instances are created via a factory pattern.

### Client

`dial()` connects to a plugin with a capability exchange handshake where both sides exchange plugin name and version. Returns a `PluginClient` with async methods matching the Plugin trait.

### Transport

Targets support three URI schemes: `tcp://`, `unix://`, and `http://`.

### Resource Encoding

Resource inputs and outputs are passed as JSON strings over the wire, not native Rust types. Plugins are responsible for deserializing inputs and serializing outputs.

## Writing Plugins

Follow the pattern in [plugin_std_random](../plugin_std_random/) or [plugin_std_artifact](../plugin_std_artifact/):

1. Implement the `Plugin` trait.
2. Create a factory that produces plugin instances.
3. Call `rtp::serve()` with the factory and a bind address.

## Related Crates

- [RTE](../rte/) — dials plugins using this protocol
- [plugin_std_random](../plugin_std_random/) — example simple plugin
- [plugin_std_time](../plugin_std_time/) — example volatile resource plugin
- [plugin_std_artifact](../plugin_std_artifact/) — example plugin with external storage
- [plugin_std_container](../plugin_std_container/) — example complex plugin with SCOP integration

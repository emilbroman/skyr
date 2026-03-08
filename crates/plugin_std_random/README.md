# Std/Random Plugin

An [RTP](../rtp/) plugin implementing the `Std/Random.Int` resource type.

## Role in the Architecture

This is one of Skyr's standard library plugins, invoked by the [RTE](../rte/) when deployments use `Random.Int` resources.

## Resource: `Std/Random.Int`

Generates a random integer within a specified range.

| | Fields |
|---|--------|
| **Inputs** | `min` (integer), `max` (integer) |
| **Outputs** | `result` (random integer in [min, max]) |

## Running

```sh
cargo run -p plugin_std_random -- --bind 0.0.0.0:50051
```

## Related Crates

- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions

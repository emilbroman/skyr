# Std/Time Plugin

An [RTP](../rtp/) plugin implementing the `Std/Time.Schedule` resource type.

## Role in the Architecture

This is one of Skyr's standard library plugins, invoked by the [RTE](../rte/) when deployments use `Time.Schedule` resources.

## Resource: `Std/Time.Schedule`

A volatile schedule resource that truncates the current time to the closest past boundary of a given duration, aligned with the Unix epoch.

| | Fields |
|---|--------|
| **Inputs** | `months` (integer), `milliseconds` (integer) |
| **Outputs** | `epochMillis` (truncated instant) |

The resource ID encodes the duration as `<months>/<milliseconds>`.

### Boundary Calculation

1. **Month alignment**: compute the largest epoch-aligned month boundary that is at or before the current time.
2. **Millisecond alignment**: from that month boundary, find the largest millisecond-aligned boundary at or before the current time.

For example, with a duration of 1 month + 1 millisecond:
- The second window starts at `1970-02-01T00:00:00.001Z`
- The third window starts at `1970-03-01T00:00:00.002Z`

### Volatile Behavior

The resource is marked `Volatile`, so the deployment engine periodically calls `check` to recompute the current window. When the schedule crosses a boundary, the `epochMillis` output changes, which triggers dependent resources to update.

## Running

```sh
cargo run -p plugin_std_time -- --bind 0.0.0.0:50056
```

## Related Crates

- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions

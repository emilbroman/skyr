# Std/DNS Plugin

An [RTP](../rtp/) plugin implementing the `Std/DNS.ARecord` resource type.

## Role in the Architecture

This is one of Skyr's standard library plugins, invoked by the [RTE](../rte/) when deployments use `DNS.ARecord` resources.

## Resource: `Std/DNS.ARecord`

Manages a DNS A record.

| | Fields |
|---|--------|
| **Inputs** | `name` (string), `ttl` (Duration), `addresses` (list of strings) |
| **Outputs** | (none beyond inputs) |

This is currently a stub plugin — it echoes inputs back without performing actual DNS provisioning.

## Running

```sh
cargo run -p plugin_std_dns -- --bind 0.0.0.0:50057
```

## Related Crates

- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions

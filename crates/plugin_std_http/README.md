# Std/HTTP Plugin

An [RTP](../rtp/) plugin implementing the `Std/HTTP.Get` resource type.

## Role in the Architecture

This is one of Skyr's standard library plugins, invoked by the [RTE](../rte/) when deployments use `HTTP.Get` resources.

## Resource: `Std/HTTP.Get`

Performs an HTTP GET request and exposes the response status, headers, and body.

| | Fields |
|---|--------|
| **Inputs** | `url` (string) |
| | `headers` (optional dict of string to string) — request headers to send |
| **Outputs** | `status` (HTTP status code) |
| | `headers` (dict of string to string) — response headers, names lowercased |
| | `body` (string) — response body |

## Running

```sh
cargo run -p plugin_std_http -- --bind 0.0.0.0:50058
```

## Related Crates

- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions

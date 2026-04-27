# Std/DNS Plugin

An [RTP](../rtp/) plugin implementing DNS resource types from `Std/DNS`. Records are stored in Redis and served by an embedded UDP DNS server.

## Role in the Architecture

This is one of Skyr's standard library plugins, invoked by the [RTE](../rte/) when deployments use `Std/DNS` resources.

## Resources

### `Std/DNS.ARecord`

Manages a DNS A record (IPv4).

| | Fields |
|---|--------|
| **Inputs** | `name` (Str), `ttl` (Duration), `addresses` ([Str]) |
| **Outputs** | `fqdn` (Str), `ttl` (Duration), `addresses` ([Str]) |

### `Std/DNS.AAAARecord`

Manages a DNS AAAA record (IPv6).

| | Fields |
|---|--------|
| **Inputs** | `name` (Str), `ttl` (Duration), `addresses` ([Str]) |
| **Outputs** | `fqdn` (Str), `ttl` (Duration), `addresses` ([Str]) |

### `Std/DNS.CNAMERecord`

Manages a DNS CNAME record.

| | Fields |
|---|--------|
| **Inputs** | `name` (Str), `ttl` (Duration), `target` (Str) |
| **Outputs** | `fqdn` (Str), `ttl` (Duration), `target` (Str) |

### `Std/DNS.TXTRecord`

Manages a DNS TXT record.

| | Fields |
|---|--------|
| **Inputs** | `name` (Str), `ttl` (Duration), `values` ([Str]) |
| **Outputs** | `fqdn` (Str), `ttl` (Duration), `values` ([Str]) |

### `Std/DNS.MXRecord`

Manages a DNS MX record.

| | Fields |
|---|--------|
| **Inputs** | `name` (Str), `ttl` (Duration), `exchanges` ([{priority: Int, host: Str}]) |
| **Outputs** | `fqdn` (Str), `ttl` (Duration), `exchanges` ([{priority: Int, host: Str}]) |

### `Std/DNS.SRVRecord`

Manages a DNS SRV record.

| | Fields |
|---|--------|
| **Inputs** | `name` (Str), `ttl` (Duration), `records` ([{priority: Int, weight: Int, port: Int, target: Str}]) |
| **Outputs** | `fqdn` (Str), `ttl` (Duration), `records` ([{priority: Int, weight: Int, port: Int, target: Str}]) |

## Running

```sh
cargo run -p plugin_std_dns -- --bind 0.0.0.0:50057 --redis-hostname localhost --zone example.internal
```

## Related Crates

- [RTP](../rtp/) — the plugin protocol this implements
- [RTE](../rte/) — invokes this plugin to process transitions

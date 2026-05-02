# Skyr Configuration Server (SCS)

SCS hosts a Git server over SSH and stores the received configuration in the [CDB](../cdb/).

## Role in the Architecture

SCS is the entry point for all user-initiated deployments. Users interact with Skyr by pushing Git commits to SCS, which handles the SSH transport, packfile parsing, and deployment state management.

```
User (Git/SSH) → SCS edge ──► IAS (auth, region-pooled)
                          ──► GDDB (look up repo home region)
                          ──► CDB at repo's home region
```

SCS edges are **region-agnostic**. Anycast lands the user at the nearest edge; the edge then routes per-channel using token-equivalent SSH pubkey checks at the user's home-region IAS, GDDB lookups for the repo's home region, and the resource's region (encoded structurally in `ResourceQid`) for port-forward. There is no SSH-to-SSH proxy to a "home" SCS — every edge talks directly to whichever region's databases the request needs.

The edge takes `--domain` (DNS suffix, e.g. `skyr.cloud`) and `--gddb-bootstrap-region` (the region whose `gddb.<region>.int.<domain>` peer to bootstrap the GDDB ScyllaDB session against). It does **not** take a `--region` flag.

## How It Works

### Authentication

SCS validates incoming SSH connections by:
1. Resolving the SSH username's home region in GDDB (usernames are personal-org names).
2. Calling `IAS.ListCredentials` at that region and checking that the connecting key's fingerprint is registered for the user.

Unknown users, unknown fingerprints, and invalid usernames are all rejected without distinguishing between them, so the edge does not leak which usernames exist.

### Push (`git-receive-pack`)

When a user pushes:
1. Parses the ref update commands from the client.
2. Parses the incoming packfile, resolves deltas, and writes Git objects (blobs, trees, commits) into CDB.
3. Converts Git refs to environment IDs using `EnvironmentId::from_git_ref()` (stripping `refs/heads/` and `refs/tags/` prefixes).
4. Marks new deployments as **Desired** (active).
5. Marks replaced deployments as **Lingering** (superseded by the new deployment).
6. Marks deleted environments as **Undesired** (scheduled for teardown).
7. Records the supersession relationship between old and new deployments.

### Fetch (`git-upload-pack`)

When a user fetches:
1. Advertises active deployments that are not in the Undesired or Lingering state as refs, reconstructing full Git ref paths via `EnvironmentId::to_git_ref()`.
2. Streams a generated packfile from stored Git objects.

## Git Protocol Capabilities

SCS implements Git pack protocol v1. The table below summarizes all capabilities relevant to the server, whether they are advertised during ref advertisement, and whether they are functionally implemented.

| Capability | Advertised (upload-pack) | Advertised (receive-pack) | Parsed from Client | Implemented | Notes |
|---|---|---|---|---|---|
| `side-band-64k` | Yes | Yes | Yes (both paths) | Yes | Upload-pack: wraps packfile data in side-band channel 1. Receive-pack: wraps report-status in side-band channel 1. Falls back to raw pkt-line if client does not request it. |
| `report-status` | No | Yes | Yes | Yes | When requested by the client, server sends `unpack ok` followed by `ok <ref>` for each updated ref after processing the push. |
| `delete-refs` | No | Yes | No | Implicit | Advertised so clients know deletions are accepted, but the server does not gate deletion handling on whether the client sent the capability — ref deletions (new OID = null) are always processed. |
| `ofs-delta` | No | Yes | N/A (pack encoding) | Yes | Pack object type 6 is fully parsed and resolved. Base objects are looked up by offset within the packfile. Advertised on receive-pack so clients know they can send ofs-delta objects. Not advertised on upload-pack because the server only generates non-delta objects. |
| `ref-delta` | No | No | N/A (pack encoding) | Yes | Pack object type 7 is fully parsed and resolved. Base objects are looked up by SHA-1 from CDB, with graceful retry when bases arrive later in the pack. Not advertised as a capability. |
| `shallow` / `deepen` | Yes | No | Yes (upload-pack) / Shallow lines skipped (receive-pack) | Yes (upload-pack) | Upload-pack: advertised and fully implemented. Parses `shallow` (client boundaries) and `deepen <n>` lines. Computes new shallow/unshallow boundaries via commit graph walk, sends them before have/done negotiation, and limits commit traversal to the requested depth during packfile generation. Receive-pack: `shallow` and `unshallow` lines are still silently discarded during command parsing. |
| `thin-pack` | No | No | No | No | Not advertised or handled. Thin packs (packs with delta bases outside the pack) may partially work via ref-delta resolution against CDB, but there is no explicit thin-pack support. |
| `no-done` | Yes | No | Yes | Yes | When negotiated together with `multi_ack_detailed`, the client may omit the `done` line once the server has sent `ACK <oid> ready`. This saves one round-trip during fetch negotiation. |
| `multi_ack_detailed` | Yes | No | Yes | Yes | During have/done negotiation, the server sends `ACK <oid> common` for each `have` OID it recognizes, and `ACK <oid> ready` after a flush when common objects have been found. After `done` (or when `no-done` applies), a final `ACK <oid>` (no suffix) is sent for the last common object. Falls back to plain `NAK`-only negotiation if the client does not request this capability. |
| `allow-tip-sha1-in-want` | No | No | No | No | Not implemented. |
| `allow-reachable-sha1-in-want` | No | No | No | No | Not implemented. |
| `agent` | No | No | No | No | Not parsed or sent. |

### Additional protocol details

- **Ref advertisement** (`advertise_refs`): Capabilities are appended after a NUL byte on the first ref line. If no refs exist, a zero-id `capabilities^{}` pseudo-ref is sent instead.
- **Upload-pack negotiation**: The server reads `want` lines, then reads `shallow`/`deepen` lines if present, computes and sends shallow/unshallow boundary updates, then processes `have`/`done` negotiation. When `multi_ack_detailed` is negotiated, the server sends `ACK <oid> common` for each `have` OID it recognizes and `ACK <oid> ready` after a flush once common objects exist, allowing the client to converge on the minimal set of objects to transfer. When `no-done` is also negotiated, the client may skip the `done` line after receiving `ready`, saving a round-trip. Without `multi_ack_detailed`, the server falls back to responding with `NAK` after each batch flush. When generating the packfile, the server walks the object graph from wanted commits and stops traversal at any `have` OID, producing an incremental pack that only contains objects the client does not already have.
- **Receive-pack command parsing**: The first command line's NUL-separated capabilities are parsed for `side-band-64k` and `report-status`. All other capabilities sent by the client are ignored.

## Related Crates

- [IDs](../ids/) — typed identifiers (RepoQid, EnvironmentId, DeploymentId) for ref conversion
- [CDB](../cdb/) — where Git objects and deployment metadata are stored
- [GDDB](../gddb/) — looks up the repo's and user's home regions
- [IAS](../ias/) — fronts UDB for SSH pubkey checks and org membership
- [DE](../de/) — picks up deployments created by SCS

## Deployment States

See [Deployments](../../docs/deployments.md) for the full lifecycle of deployment states managed by SCS and DE.

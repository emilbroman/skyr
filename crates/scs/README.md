# Skyr Configuration Server (SCS)

SCS hosts a Git server over SSH and stores the received configuration in the [CDB](../cdb/).

## Role in the Architecture

SCS is the entry point for all user-initiated deployments. Users interact with Skyr by pushing Git commits to SCS, which handles the SSH transport, packfile parsing, and deployment state management.

```
User (Git/SSH) → SCS → CDB (store objects + deployments)
                       UDB (authenticate)
```

## How It Works

### Authentication

SCS validates incoming SSH connections by checking:
1. The SSH username exists in the [UDB](../udb/) (user database).
2. The connecting key's fingerprint is present in that user's stored public key set.

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
| `shallow` / `deepen` | No | No | Shallow lines skipped | No | `shallow` and `unshallow` lines from the client are recognized and silently discarded during receive-pack command parsing (line 617). No shallow clone support is implemented. |
| `thin-pack` | No | No | No | No | Not advertised or handled. Thin packs (packs with delta bases outside the pack) may partially work via ref-delta resolution against CDB, but there is no explicit thin-pack support. |
| `no-done` | No | No | No | No | Not supported. The upload-pack path waits for an explicit `done` line from the client before sending the packfile. |
| `multi_ack` / `multi_ack_detailed` | No | No | No | No | Not implemented. The server sends a single `NAK` after receiving wants and the `done` line, with no `have` negotiation. |
| `allow-tip-sha1-in-want` | No | No | No | No | Not implemented. |
| `allow-reachable-sha1-in-want` | No | No | No | No | Not implemented. |
| `agent` | No | No | No | No | Not parsed or sent. |

### Additional protocol details

- **Ref advertisement** (`advertise_refs`): Capabilities are appended after a NUL byte on the first ref line. If no refs exist, a zero-id `capabilities^{}` pseudo-ref is sent instead.
- **Upload-pack negotiation**: The server reads `want` lines, then reads through to a `done` line (ignoring `have` lines), responds with `NAK`, and streams the full packfile. There is no common-ancestor negotiation — every fetch sends all reachable objects.
- **Receive-pack command parsing**: The first command line's NUL-separated capabilities are parsed for `side-band-64k` and `report-status`. All other capabilities sent by the client are ignored.

## Related Crates

- [IDs](../ids/) — typed identifiers (RepoQid, EnvironmentId, DeploymentId) for ref conversion
- [CDB](../cdb/) — where Git objects and deployment metadata are stored
- [UDB](../udb/) — user authentication data
- [DE](../de/) — picks up deployments created by SCS

## Deployment States

See [Deployments](../../docs/deployments.md) for the full lifecycle of deployment states managed by SCS and DE.

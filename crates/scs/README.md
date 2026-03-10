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
7. Records the supercession relationship between old and new deployments.

### Fetch (`git-upload-pack`)

When a user fetches:
1. Advertises active deployments that are not in the Undesired or Lingering state as refs, reconstructing full Git ref paths via `EnvironmentId::to_git_ref()`.
2. Streams a generated packfile from stored Git objects.

## Related Crates

- [IDs](../ids/) — typed identifiers (RepoQid, EnvironmentId, DeploymentId) for ref conversion
- [CDB](../cdb/) — where Git objects and deployment metadata are stored
- [UDB](../udb/) — user authentication data
- [DE](../de/) — picks up deployments created by SCS

## Deployment States

See [Deployments](../../docs/deployments.md) for the full lifecycle of deployment states managed by SCS and DE.

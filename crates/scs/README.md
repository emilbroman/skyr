# Skyr Configuration Server (SCS)

SCS hosts a Git server over SSH and stores the received configuration in the [CDB](../cdb/).

## Role in the Architecture

SCS is the entry point for all user-initiated deployments. Users interact with Skyr by pushing Git commits to SCS, which handles the SSH transport, packfile parsing, and deployment state management.

```
User (Git/SSH) → SCS → CDB (store objects + deployments)
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
3. Marks new refs as **Desired** (active deployment).
4. Marks replaced refs as **Lingering** (superseded by the new deployment).
5. Marks deleted refs as **Undesired** (scheduled for teardown).
6. Records the supercession relationship between old and new deployments.

### Fetch (`git-upload-pack`)

When a user fetches:
1. Advertises active deployments that are not in the Undesired or Lingering state as refs.
2. Streams a generated packfile from stored Git objects.

## Related Crates

- [CDB](../cdb/) — where Git objects and deployment metadata are stored
- [UDB](../udb/) — user authentication data
- [DE](../de/) — picks up deployments created by SCS

## Deployment States

See [Deployments](../../docs/deployments.md) for the full lifecycle of deployment states managed by SCS and DE.

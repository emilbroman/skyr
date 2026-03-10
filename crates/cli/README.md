# Skyr CLI

The CLI binary (`skyr`) provides a command-line interface and REPL for working with Skyr.

## Role in the Architecture

The CLI is the primary user-facing tool for interacting with Skyr. It uses [SCLC](../sclc/) for local configuration evaluation and the [API](../api/) for remote operations.

## Global Flags

| Flag | Description |
|------|-------------|
| `--format` | Output format: `text` (default) or `json` |

## Commands

| Command | Description |
|---------|-------------|
| `repl` | Interactive SCL REPL |
| `run` | Execute an SCL program locally |
| `signup` | Create a new user account |
| `signin` | Authenticate and get a bearer token |
| `whoami` | Show the current authenticated user |
| `repo list` | List repositories |
| `repo create <org/repo>` | Create a new repository |
| `deployments list <org/repo>` | List deployments for a repository |
| `deployments logs <org/repo>` | Stream deployment logs for a repository |

### Command Details

**`run`** accepts `--root` (default: `.`) to set the project root and `--package` (default: `Local`) to set the package name.

**`signup`** and **`signin`** accept `--key` (default: `~/.ssh/id_ed25519`) for the SSH key path and `--api_url` (default: `localhost:8080`) for the API endpoint. `signup` additionally requires `--username` and `--email`.

**`deployments logs`** streams logs in real time via WebSocket.

## Related Crates

- [SCLC](../sclc/) — SCL compiler used for `repl` and `run` commands
- [API](../api/) — GraphQL API used for account and deployment commands

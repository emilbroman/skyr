# Skyr CLI

The CLI binary (`skyr`) provides a command-line interface and REPL for working with Skyr.

## Role in the Architecture

The CLI is the primary user-facing tool for interacting with Skyr. It uses [SCLC](../sclc/) for local configuration evaluation and the [API](../api/) for remote operations.

## Commands

| Command | Description |
|---------|-------------|
| `repl` | Interactive SCL REPL |
| `run` | Execute an SCL program locally |
| `signup` | Create a new user account |
| `signin` | Authenticate and get a bearer token |
| `whoami` | Show the current authenticated user |
| `repo` | Manage repositories |
| `deployments` | View deployment status |

## Related Crates

- [SCLC](../sclc/) — SCL compiler used for `repl` and `run` commands
- [API](../api/) — GraphQL API used for account and deployment commands

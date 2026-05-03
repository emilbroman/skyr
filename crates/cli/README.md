# Skyr CLI

The CLI binary (`skyr`) provides a command-line interface and REPL for working with Skyr.

## Role in the Architecture

The CLI is the primary user-facing tool for interacting with Skyr. It uses [SCLC](../sclc/) for local configuration evaluation and the [API](../api/) for remote operations.

## Global Flags

These are accepted on every subcommand and may also be set via environment variable.

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--format` |  | `text` | Output format: `text` or `json` |
| `--api-url` | `SKYR_API_URL` | `https://skyr.cloud` | Skyr API base URL (point at any region's edge — token claims and GDDB lookups handle routing from there) |
| `--org` | `SKYR_ORG` | parsed from `skyr` (or `origin`) git remote | Override the organization |
| `--repo` | `SKYR_REPO` | parsed from the same remote | Override the repository |
| `--env` | `SKYR_ENV` | current git branch | Override the environment |

## Commands

| Command | Description |
|---------|-------------|
| `repl` | Interactive SCL REPL |
| `run` | Execute `Main.scl` locally |
| `fmt` | Format an SCL file |
| `lsp` | Start the SCL language server (LSP over stdin/stdout) |
| `auth signup` | Create a new user account |
| `auth signin` | Authenticate and store a bearer token |
| `auth whoami` | Show the current authenticated user |
| `auth signout` | Discard the stored token |
| `org list` | List organizations the authenticated user belongs to |
| `org create <name>` | Create a new organization |
| `org add-member <org> <user>` | Add a user to an organization |
| `org leave <org>` | Leave an organization |
| `repo list` | List repositories in the current organization |
| `repo create <name>` | Create a new repository |
| `deployments list` | List deployments for the current repository |
| `deployments logs` | Stream deployment logs (WebSocket) |
| `resources list` | List resources for the current environment |
| `resources logs <qid>...` | Show or follow logs for one or more resources |
| `resources delete <type:name>` | Mark a resource for teardown |
| `port-forward` | Forward a local port to a resource exposed via the SCS edge |
| `deps list` / `deps add` / `deps rm` | Manage cross-repo dependencies in `Package.scle` |
| `api query <body>` | Direct GraphQL query — see `docs/index.md` for argument-spec rules |
| `api mut <body>` | Direct GraphQL mutation |

### Command details

**`run`** accepts `--root` (default: `.`), `--package` (default: `Local`), and `--git-server` (default: `skyr.cloud:22`).

**`auth signup`** requires `--username`, `--email`, and `--region` (`[a-z]+`, the metro your user record will live in). It accepts `--key` (default: `~/.ssh/id_ed25519`) and `--fullname` (optional). The chosen region is the home region of your personal organisation — it determines which IAS owns your identity and which UDB stores your credentials. Other regions can verify tokens you sign in with via the issuer region's published public key.

**`auth signin`** accepts `--key` and (for new keys whose home region the edge cannot infer from a stored credential) `--region`.

**`org create`** and **`repo create`** accept an optional `--region`; when omitted, the new entity inherits the creator's region (org) or the org's region (repo). The chosen region is recorded in [GDDB](../gddb/) at create time and is used to route all subsequent reads.

**`deployments logs`** streams logs in real time via WebSocket.

## Related Crates

- [SCLC](../sclc/) — SCL compiler used for `repl` and `run` commands
- [API](../api/) — GraphQL API used for account, deployment, and resource commands

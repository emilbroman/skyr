# Skyr Web

The web dashboard provides a browser-based UI for browsing organizations, repositories, environments, deployments, resources, logs, and source trees.

## Role in the Architecture

The web app is the graphical frontend for Skyr. It communicates exclusively with the [API](../crates/api/) service over GraphQL (HTTP for queries/mutations, WebSocket for subscriptions).

```
Browser → Web (SvelteKit SPA) → /graphql → API edge → IAS, CDB, RDB, ADB, LDB, SDB
```

The API edge is region-agnostic: it routes per-request to the user's home region via [GDDB](../crates/gddb/), and all auth flows go through [IAS](../crates/ias/) (which owns UDB internally). The browser never talks to IAS or UDB directly.

In development, Vite proxies `/graphql` requests to `http://localhost:8080` (the API service). In production the app is deployed as static files behind a reverse proxy that routes `/graphql` to the API.

## Tech Stack

| Layer               | Technology                                                          |
| ------------------- | ------------------------------------------------------------------- |
| Framework           | [SvelteKit 2](https://kit.svelte.dev/) (static adapter, CSR-only)   |
| UI                  | [Svelte 5](https://svelte.dev/)                                     |
| Styling             | [Tailwind CSS 4](https://tailwindcss.com/)                          |
| Build               | [Vite 6](https://vite.dev/)                                         |
| Language            | TypeScript 5                                                        |
| GraphQL             | `graphql` + `graphql-ws` (WebSocket subscriptions)                  |
| Code generation     | `@graphql-codegen` (typed document nodes from `.graphql` files)     |
| Syntax highlighting | [Shiki](https://shiki.matsu.io/) with a custom SCL TextMate grammar |

## Project Structure

```
web/
├── package.json
├── vite.config.ts            # Vite config (Tailwind plugin, /graphql proxy)
├── svelte.config.js          # SvelteKit config (static adapter)
├── codegen.ts                # GraphQL codegen config
├── tsconfig.json
├── static/                   # Static assets
└── src/
    ├── app.html              # HTML shell
    ├── app.css               # Tailwind CSS import
    ├── app.d.ts              # SvelteKit ambient types
    ├── routes/
    │   ├── +layout.svelte    # Root layout (auth state)
    │   ├── +layout.ts        # Disables SSR (CSR-only)
    │   ├── ~signin/          # Sign-in page
    │   └── (app)/            # Authenticated route group
    │       ├── +layout.svelte        # App shell (sidebar, nav)
    │       ├── +page.svelte          # Organizations list
    │       ├── [org]/                # Org detail
    │       ├── [org]/[repo]/         # Repo detail
    │       ├── [org]/[repo]/[env]/   # Environment detail
    │       └── [org]/[repo]/[env]/[deployment]/  # Deployment detail
    └── lib/
        ├── components/
        │   ├── DeploymentState.svelte  # Deployment status badge
        │   ├── FileBrowser.svelte      # Source tree and blob viewer
        │   ├── LogStream.svelte        # Real-time log streaming
        │   └── ResourceCard.svelte     # Resource detail card
        ├── graphql/
        │   ├── client.ts               # GraphQL HTTP + WebSocket client
        │   ├── ws.ts                   # WebSocket subscription handler
        │   ├── generated.ts            # Codegen output (do not edit)
        │   └── documents/              # .graphql query/mutation/subscription files
        ├── stores/
        │   └── auth.ts                 # Token and user state (localStorage)
        ├── highlight.ts                # Shiki syntax highlighter setup
        ├── scl.tmLanguage.json         # SCL TextMate grammar for Shiki
        ├── format.ts                   # Value formatting utilities
        └── paths.ts                    # URL path builders/decoders
```

## Pages

| Route                                | Description                                                                  |
| ------------------------------------ | ---------------------------------------------------------------------------- |
| `~signup`                            | New-account sign-up (username, email, region, plus SSH or WebAuthn proof)    |
| `~signin`                            | SSH or WebAuthn challenge-response sign-in                                   |
| `/`                                  | Organizations dashboard                                                      |
| `[org]`                              | Repositories for an organization                                             |
| `[org]/~i`                           | Incidents view: all incidents in the org, filterable by category and status  |
| `[org]/~i/[id]`                      | Incident detail with links back to the owning deployment / resource          |
| `[org]/[repo]`                       | Environments for a repository                                                |
| `[org]/[repo]/[env]`                 | Environment shell with tabbed sub-pages (deployments / resources / logs / artifacts) |
| `[org]/[repo]/[env]/~d/[deployment]` | Deployment detail: status, resources, logs, source tree, artifacts           |
| `[org]/[repo]/[env]/~r/[resource]`   | Resource detail: status, inputs, outputs, dependencies, incidents            |

## Authentication

The web app uses the same challenge-response flow as the [CLI](../crates/cli/), supporting both SSH signatures and WebAuthn passkeys:

1. **Sign-up** — user provides username, email, **region** (the metro their account will live in), and either pastes an SSH signature over the displayed challenge or completes a WebAuthn passkey registration. The selected region determines which [IAS](../crates/ias/) owns their identity and signs their tokens.
2. **Sign-in** — user provides their username; the app requests a challenge from `authChallenge` and the user proves possession of their key (SSH or passkey).
3. The app calls `signup` / `signin` and stores the bearer identity token in `localStorage`.
4. Tokens auto-refresh ~2 minutes before expiry. Cross-region API calls are transparent to the browser: any edge can verify a token using the issuer region's IAS-published public key.

## GraphQL Integration

GraphQL operations are defined as `.graphql` files in `src/lib/graphql/documents/`:

| File                    | Operations                         |
| ----------------------- | ---------------------------------- |
| `auth.graphql`          | Challenge, sign-in, token refresh  |
| `organizations.graphql` | Organization listing and detail    |
| `repositories.graphql`  | Repository listing and detail      |
| `environment.graphql`   | Environment and deployment queries (including `status` and `incidents` for deployments and resources) |
| `incidents.graphql`     | Org-scoped incident listing and single-incident detail |
| `logs.graphql`          | Log subscriptions (WebSocket)      |
| `tree.graphql`          | File tree and blob content         |

Running `npm run codegen` reads the API schema from `../crates/api/schema.graphql` and generates typed document nodes into `src/lib/graphql/generated.ts`. Re-run this whenever the API schema changes.

## Development

### Prerequisites

- Node.js (LTS)
- The [API](../crates/api/) service running on `localhost:8080` (Vite proxies `/graphql` to it)

### Getting Started

```sh
cd web
npm install
npm run dev          # Start dev server with hot reload
```

The dev server starts on `http://localhost:5173` by default.

### Available Scripts

| Script            | Description                                             |
| ----------------- | ------------------------------------------------------- |
| `npm run dev`     | Start Vite dev server with hot reload and GraphQL proxy |
| `npm run build`   | Production build (static files output to `build/`)      |
| `npm run preview` | Preview production build locally                        |
| `npm run check`   | Run svelte-check for type errors                        |
| `npm run codegen` | Regenerate TypeScript types from GraphQL schema         |

### Regenerating GraphQL Types

When the API schema changes:

1. Regenerate the schema file: `cargo run -p api -- --write-schema`
2. Regenerate the TypeScript types: `npm run codegen`

### URL Path Encoding

Environment and ref names may contain slashes. Since some reverse proxies (e.g. Traefik) reject percent-encoded slashes in URL paths, the app encodes `/` as `~` in route parameters. The helpers in `src/lib/paths.ts` handle this encoding.

## Components

| Component         | Description                                                                   |
| ----------------- | ----------------------------------------------------------------------------- |
| `DeploymentState` | Colored badge for deployment states (DESIRED, LINGERING, UNDESIRED, DOWN)     |
| `HealthBadge`     | Colored badge for `StatusSummary.health` (HEALTHY, DEGRADED, DOWN) on deployments and resources |
| `FileBrowser`     | Recursive tree/blob display for browsing a deployment's source files          |
| `LogStream`       | Real-time log viewer via WebSocket subscription with auto-scroll              |
| `ResourceCard`    | Collapsible card showing a resource's type, inputs, outputs, and dependencies |

## Related Crates

- [API](../crates/api/) — GraphQL backend the web app consumes
- [CLI](../crates/cli/) — command-line interface (shares the same authentication flow)
- [IDs](../crates/ids/) — namespace hierarchy reflected in the URL structure

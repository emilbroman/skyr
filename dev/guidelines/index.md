# Developer Guidelines

This directory collects guidelines for developers working on the Skyr codebase.

The intent is to capture the conventions, patterns, and trade-offs that aren't obvious from reading the code alone — the kind of context that usually only lives in reviewers' heads or in scattered PR discussions. Where the [README](../../README.md) and crate-level docs describe *what* Skyr is and *how* the pieces fit together, these guidelines describe *how we work* on it: stylistic choices, architectural defaults, things to reach for, and things to avoid.

## Topics

- [Architecture](architecture.md) — horizontal scalability, sharding, coherence, push/pull handoffs, queue vs gRPC.
- [Boundaries](boundaries.md) — boundaries live in code (libraries) not infrastructure, majestic monolith, data ownership by crate.
- [Storage](storage.md) — component-owned vs first-class handoff storage, race-free writes, cross-DB joins, choosing new backing stores.
- [Component Naming](naming.md) — short names as keystroke economy, the surface pattern, when to skip the abbreviation.
- [Rust Style](rust-style.md) — error handling, async/sync defaults, newtypes (ongoing).

## Audit Log

This table tracks when each component was last audited for compliance with the guidelines above.

| Component             | Last audit date |
| --------------------- | --------------- |
| de                    | 2026-04-29 @ f9fd633 |
| adb                   |                 |
| api                   |                 |
| cdb                   |                 |
| cli                   |                 |
| ids                   |                 |
| ldb                   |                 |
| lsp                   |                 |
| ne                    |                 |
| nq                    |                 |
| plugin_std_artifact   |                 |
| plugin_std_container  |                 |
| plugin_std_crypto     |                 |
| plugin_std_dns        |                 |
| plugin_std_http       |                 |
| plugin_std_random     |                 |
| plugin_std_time       |                 |
| rdb                   |                 |
| re                    |                 |
| rq                    |                 |
| rte                   |                 |
| rtp                   |                 |
| rtq                   |                 |
| sclc                  |                 |
| sclc-docgen           |                 |
| sclc-wasm             |                 |
| scoc                  |                 |
| scop                  |                 |
| scs                   |                 |
| sdb                   |                 |
| udb                   |                 |
| web                   |                 |

### Updating the audit log

To audit a component (taking `de` as an example):

1. Find the commit hash of when the component was last changed:

   ```sh
   git log -1 --format=%h -- crates/de
   ```

   For `web`, use `git log -1 --format=%h -- web` instead.

2. Look up the current date:

   ```sh
   date +%Y-%m-%d
   ```

3. Record both in the "Last audit date" column as `<ISO date> @ <short hash>` (e.g. `2026-04-29 @ 3daaa01`).

4. Move the row for the just-audited component to the top of the table. (The table is kept sorted by audit recency in descending order, and since you've just performed the most recent audit, simply moving this one row preserves that ordering — no need to re-sort the rest.)

After updating the column, audit the component against the guidelines in this directory. Address any violations or critique immediately, or file them as GitHub issues.

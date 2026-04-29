# Developer Guidelines

This directory collects guidelines for developers working on the Skyr codebase.

The intent is to capture the conventions, patterns, and trade-offs that aren't obvious from reading the code alone — the kind of context that usually only lives in reviewers' heads or in scattered PR discussions. Where the [README](../../README.md) and crate-level docs describe *what* Skyr is and *how* the pieces fit together, these guidelines describe *how we work* on it: stylistic choices, architectural defaults, things to reach for, and things to avoid.

## Topics

- [Architecture](architecture.md) — horizontal scalability, sharding, coherence, push/pull handoffs, queue vs gRPC.
- [Boundaries](boundaries.md) — boundaries live in code (libraries) not infrastructure, majestic monolith, data ownership by crate.
- [Storage](storage.md) — component-owned vs first-class handoff storage, race-free writes, cross-DB joins, choosing new backing stores.
- [Component Naming](naming.md) — short names as keystroke economy, the surface pattern, when to skip the abbreviation.
- [Rust Style](rust-style.md) — error handling, async/sync defaults, newtypes (ongoing).

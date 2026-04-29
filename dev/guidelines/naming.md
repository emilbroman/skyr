# Component Naming

Convention, not guideline. Don't put too much weight on this.

## Why the Short Names Exist

The only reason Skyr uses short component names like DE, RTE, CDB, RTQ is **keystroke economy**. Writing "deployment engine" every time we talk about that component is messy. If everyone knows what the DE is, the shorthand is efficient — that is the whole story.

## Surface Pattern

In practice the existing components fall into a recognizable shape — short functional prefix plus a type-letter suffix:

- `DB` for databases (CDB, RDB, SDB, UDB, ADB, LDB)
- `Q` for queues (RTQ, RQ, NQ)
- `E` for engines (DE, RTE, RE, NE)
- `P` for protocols (RTP, SCOP)
- `S` for servers (SCS)
- `C` for compiler / conduit (SCLC, SCOC)

The expectation is mild: the next database crate that is large enough to warrant its own crate would probably end up with an `xdb` name, because that's already the pattern. But it isn't a rule — pick the name that reads well.

## When to Skip the Abbreviation

Plenty of crates don't follow the pattern, and that's fine. The opt-outs come in two flavors:

- **Following a different convention**, like `plugin_*` / `plugin_std_*`. The plugin family has its own naming shape and doesn't need to inherit the abbreviation pattern.
- **Not central enough to warrant the keystroke optimization** — `ids`, `web`, `cli`, `api`. We don't talk about these often enough, in long-form prose, for a short name to pay for itself.

No biggie either way. If a component's name is already short and clear, leave it alone.

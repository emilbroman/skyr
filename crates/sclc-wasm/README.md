# SCLC WASM

`sclc-wasm` is a `cdylib` crate that compiles [SCLC](../sclc/) to WebAssembly and exposes a thin `wasm-bindgen` surface for the web frontend. It powers the in-browser SCL playground, the documentation editor previews, and Monaco-based language features (diagnostics, hover, completion, go-to-definition, formatting, REPL) without a backend round-trip.

## Role in the Architecture

The crate is built with `wasm-pack` and the resulting bundle is loaded inside a Web Worker (`web/src/lib/playground/worker.ts`). The worker calls the exported functions to type-check, format, and evaluate SCL source entirely in the browser.

```
web frontend → Web Worker → sclc-wasm (wasm-bindgen) → SCLC
```

`sclc-wasm` depends on `sclc` with `default-features = false` so it links the compiler core without any host-only features.

## Exported Functions

All functions are `#[wasm_bindgen]` and operate on a `files_json` parameter — a JSON object mapping workspace-relative file paths (e.g. `models/User.scl`) to file contents. Files are loaded into an `InMemoryPackage` rooted at the `Local` package ID; module IDs are derived from path segments, so `models/User.scl` becomes `Local/models/User`.

| Function | Description |
|----------|-------------|
| `analyze(files_json)` | Builds an ASG over every `.scl` and `.scle` file in the workspace, type-checks it, and returns diagnostics as a JSON array of `{ file, line, character, end_line, end_character, message, severity }`. Filters to the `Local` package only. |
| `hover(files_json, file, line, col)` | Returns hover info (`{ type, description? }`) at a position, or `null` if no information is available. |
| `completions(files_json, file, line, col)` | Returns a JSON array of completion items (`{ label, kind, detail?, description? }`). Kinds map SCLC `CompletionCandidate` variants to `variable`, `field`, `module`, `folder`, or `file`. |
| `goto_definition(files_json, file, line, col)` | Returns the declaration location (`{ file?, line, character, end_line, end_character }`) or `null`. |
| `format(source)` | Formats a single SCL file. Returns `null` when the source is already formatted. |
| `format_scle(source)` | Formats a single SCLE file. Returns `null` when unchanged or when the source fails to parse. |
| `analyze_scle(source)` | Type-checks a standalone SCLE source string and returns diagnostics in the same shape as `analyze`. |
| `repl_init()` / `repl_reset()` | Initializes or resets the per-thread REPL session. |
| `repl_eval(files_json, line)` | Evaluates one REPL line against the current workspace files and returns `{ output?, effects?, error? }` as JSON. |

Positions on the JS side are zero-based; the wrappers translate to/from SCLC's one-based `Position` internally. Coordinates and ranges are emitted in a shape directly compatible with Monaco.

### Workspace Loading

`load_workspace` mirrors the LSP's `analyze_workspace`: it walks every `.scl` and `.scle` file in `files_json` and resolves it as an entry point, rather than only what's reachable from `Local/Main`. This ensures unimported `.scle` files (which cannot export) still get type-checked. Module IDs are mapped back to file paths by probing for `<path>.scl` first, then `<path>.scle`, so diagnostics on `.scle` modules route to the correct editor file.

### REPL State

The REPL session is held in a thread-local `RefCell<Option<WasmReplState>>` that wraps `sclc::Repl` plus an unbounded effect channel. Each `repl_eval` swaps the user package for the latest `files_json`, drains pending effects, processes the line, and serializes the outcome (`Binding`, `Value`, `TypeDef`, `Import`, or error). `repl_init` must be called before the first `repl_eval`.

## Building

The wasm bundle is produced with `wasm-pack` targeting the web:

```sh
wasm-pack build crates/sclc-wasm --target web --out-dir ../../web/src/lib/sclc-wasm
```

The output directory is gitignored under `web/src/lib/sclc-wasm/` and consumed by the worker via `import init, { ... } from "$lib/sclc-wasm/sclc_wasm.js"`.

## Where It's Invoked

| Location | Purpose |
|----------|---------|
| `web/package.json` (`build:wasm`) | Local development helper that runs `wasm-pack build` into `web/src/lib/sclc-wasm/`. |
| `dev/Containerfile.web` | Builds the wasm bundle in a `rust:1-bookworm` stage and copies it into the web image before `npm run build`. |
| `.github/workflows/ci.yml` | Builds the wasm bundle before running web checks and builds. |
| `web/src/lib/playground/worker.ts` | Loads `sclc_wasm.js` inside a Web Worker and dispatches messages from the main thread. |

`dev/Containerfile.skyr` excludes `sclc-wasm` from the Cargo workspace at build time, since it's only needed for the web image.

## Related Crates

- [SCLC](../sclc/) — compiler whose API is re-exported through `wasm-bindgen`
- [LSP](../lsp/) — sibling consumer of SCLC's analysis APIs (hover, completion, go-to-definition); the workspace-loading and cursor-info patterns are shared
- [SCLC Docgen](../sclc-docgen/) — companion tool that emits stdlib type information as JSON for the same web frontend

# SCLC Docgen

`sclc-docgen` is a small command-line tool that compiles the SCL standard library and emits its type information as JSON. The web frontend consumes this JSON to render type-aware documentation, hover information, and code intelligence for stdlib symbols.

## Role in the Architecture

The tool wraps [SCLC](../sclc/)'s `stdlib_types()` entry point and serializes the result to a file on disk. The web frontend imports the resulting JSON at build time.

```
sclc-docgen → stdlib-types.json → web frontend (stdlib.ts)
```

## Usage

```sh
cargo run -p sclc-docgen -- -f <output-file>
```

| Flag | Description |
|------|-------------|
| `-f`, `--output-file` | Path to write the generated JSON file. |

The tool exits with a panic if compiling the standard library fails or if the output file cannot be written.

## How It Works

1. Calls `sclc::stdlib_types()`, which assembles a synthetic `Main.scl` that imports every bundled `Std/*` module and runs the full SCLC pipeline (parse, resolve, type-check).
2. For each module whose value-level export type is a record, captures both the value-level export record and the type-level export record.
3. Serializes the modules into a sorted (`BTreeMap`) JSON object keyed by module ID (e.g. `Std/Time`, `Std/Random`).
4. Writes the pretty-printed JSON to the path provided via `--output-file`.

Each module entry has the shape:

```json
{
  "Std/Random": {
    "value_exports": { "fields": { ... }, "doc_comments": { ... } },
    "type_exports":  { "fields": { ... }, "doc_comments": { ... } }
  }
}
```

The `RecordType` shape mirrors the one defined in `sclc` and is consumed on the TypeScript side by `web/src/lib/stdlib.ts`.

## Where It's Invoked

| Location | Purpose |
|----------|---------|
| `web/package.json` (`build:stdlib-types`) | Local development helper that regenerates `web/src/lib/stdlib-types.json`. |
| `dev/Containerfile.web` | Generates `stdlib-types.json` during the web image build, before the `npm run build` stage. |
| `.github/workflows/ci.yml` | Generates `web/src/lib/stdlib-types.json` before running web checks and builds. |

The generated `stdlib-types.json` is treated as a build artifact — it is regenerated whenever the stdlib changes and is not edited by hand.

## Related Crates

- [SCLC](../sclc/) — compiler whose `stdlib_types()` API powers this tool
- [SCLC WASM](../sclc-wasm/) — companion artifact that compiles SCLC for the web frontend

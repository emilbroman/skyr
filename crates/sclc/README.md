# Skyr Configuration Language Compiler (SCLC)

SCLC implements the SCL compiler and runtime, exposing an API for compiling and executing SCL programs.

## Role in the Architecture

SCLC is used by the [DE](../de/) to compile deployment configuration and by the [CLI](../cli/) to provide a REPL and local execution. It takes SCL source files (starting from `Main.scl`) and produces a typed, evaluated program.

```
DE → SCLC (compile Main.scl)
CLI → SCLC (REPL / run)
```

## Components

| Component | Description |
|-----------|-------------|
| **Lexer** | Tokenizes SCL source text |
| **Parser** | PEG parser producing AST nodes with source spans |
| **AST** | Type and value model for the language |
| **Type Checker** | Static type analysis |
| **Evaluator** | Executes the typed AST |
| **Package System** | Opens packages and resolves imports |

## Compilation Pipeline

The `compile()` function:
1. Opens `Main.scl` from the provided source (a `SourceRepo` implementation).
2. Resolves imports across packages.
3. Type checks the program.
4. Returns `Diagnosed<Program<_>>` with accumulated diagnostics.

Parse functions return `Diagnosed<Option<_>>` and report syntax errors as diagnostics (not panics or `Result` errors).

## Extern Declarations

SCL supports `extern` declarations to bind built-in (native) functions into the language. This is an internal mechanism used by the standard library to expose Rust-implemented functions to SCL code:

```scl
export let toJson = extern "Std/Encoding.toJson": fn(Any) Str
```

The string after `extern` is the internal function name, which must match a name registered via `register_extern` in the corresponding Rust standard library module. The type annotation after the colon declares the function's signature for the type checker.

Extern functions are registered in `src/std/` modules and wired into the compiler via the `std_modules!` macro in `src/std/mod.rs`.

## Key Types

- **`Diagnosed<T>`** — wraps a value with a `DiagList` of accumulated warnings and errors. Used throughout the pipeline to collect diagnostics without aborting.
- **`Program<S>`** — a compiled and type-checked program ready for evaluation. Call `.evaluate()` to execute it.
- **`SourceRepo`** — trait for providing source files to the compiler. CDB implements this for deployment compilation; the CLI implements it for local file access.
- **`Effect`** — emitted during evaluation when the program creates or modifies resources (`CreateResource`, `UpdateResource`, `TouchResource`).

## Test Harness

SCLC includes a fixture-based integration test harness that compiles and evaluates SCL programs end-to-end. Each test case is a directory under `src/tests/`:

```
src/tests/
├── mod.rs                       # Harness logic and test_case! macro invocations
├── BasicExport/
│   ├── Main.scl                 # Required: entry point
│   └── exports.txt              # Optional: expected exported value
├── ImportModule/
│   ├── Main.scl
│   ├── Other.scl                # Additional modules for import testing
│   └── exports.txt
├── DiagUndefinedVar/
│   ├── Main.scl
│   └── diag.log                 # Optional: expected diagnostics
└── RandomIntUpdate/
    ├── Main.scl
    ├── rdb.json                 # Optional: pre-existing resources
    ├── exports.txt
    └── effects.log              # Optional: expected effects
```

### Fixture Files

| File | Required | Description |
|------|----------|-------------|
| `Main.scl` | Yes | Entry point for the test case. |
| `*.scl` | No | Additional modules. Files can import each other via the directory name (e.g., `import ImportModule/Other`). |
| `diag.log` | No | Expected diagnostics, one per line in the format `ModuleId Span: message` (e.g., `DiagUndefinedVar/Main 1:16,1:17: undefined variable: y`). Missing file expects zero diagnostics. |
| `exports.txt` | No | Expected `Value::to_string()` of the record exported from `Main.scl` (e.g., `{x: 42}`). Missing file expects `{}` (empty record). |
| `effects.log` | No | Expected effects, one per line in compact format (e.g., `CreateResource ty=Std/Random.Int id=seed inputs={max: 100, min: 0}`). Missing file expects zero effects. |
| `rdb.json` | No | Pre-existing resources to load into the runtime before evaluation. Used to test update/touch effects. |

### rdb.json Schema

```json
{
  "resources": {
    "<resource-type>": {
      "<resource-id>": {
        "inputs": { ... },
        "outputs": { ... },
        "markers": ["Volatile", "Sticky"],
        "dependencies": [{ "type": "<resource-type>", "name": "<resource-name>" }]
      }
    }
  }
}
```

The `dependencies` field is optional. When present, it provides the list of resource IDs that this resource depends on, allowing the test harness to distinguish `TouchResource` from `UpdateResource` effects for resources with dependencies.

### Adding a Test Case

1. Create a directory under `src/tests/` with an UpperCamelCase name.
2. Add `Main.scl` and any expectation files.
3. Add `test_case!(YourTestName);` to `src/tests/mod.rs`.
4. Run `cargo test -p sclc -- tests::YourTestName` to verify.

### How It Works

The `test_case!` macro generates a `#[tokio::test]` function per fixture. The harness:

1. Loads `.scl` files into an in-memory `SourceRepo` with the directory name as the package ID, so cross-file imports resolve naturally (e.g., `import TestCase/Other`).
2. Compiles with `compile()` and checks diagnostics against `diag.log`.
3. If there are no errors, evaluates `Main.scl` and checks exported values against `exports.txt`.
4. Collects emitted effects and checks them against `effects.log`.

## Related Crates

- [DE](../de/) — compiles deployment configs using SCLC
- [CLI](../cli/) — provides REPL and local execution

For the SCL language reference, see [SCL Documentation](../../docs/scl/index.md).

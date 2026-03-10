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

## Key Types

- **`Diagnosed<T>`** — wraps a value with a `DiagList` of accumulated warnings and errors. Used throughout the pipeline to collect diagnostics without aborting.
- **`Program<S>`** — a compiled and type-checked program ready for evaluation. Call `.evaluate()` to execute it.
- **`SourceRepo`** — trait for providing source files to the compiler. CDB implements this for deployment compilation; the CLI implements it for local file access.
- **`Effect`** — emitted during evaluation when the program creates or modifies resources (`CreateResource`, `UpdateResource`, `TouchResource`).

## Related Crates

- [DE](../de/) — compiles deployment configs using SCLC
- [CLI](../cli/) — provides REPL and local execution

For the SCL language reference, see [SCL Documentation](../../docs/scl/index.md).

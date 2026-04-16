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

## Propositional Type Refinement

The type checker implements flow-sensitive type narrowing via propositional logic. Expressions produce propositions about their operands, and control flow constructs (currently `if`) introduce assumptions that trigger refinements through forward-chaining derivation.

### Core Concepts

**TypeId.** Every `Type` carries a `TypeId` (`usize`) that tracks the *origin of a value*, not the binding name. IDs are freshly minted at construction sites (literals, operator results, type annotations). Variable references and assignments propagate the TypeId from their source — they do not mint new IDs. This means aliased bindings (`let y = x`) share a TypeId, so refining one refines both.

TypeId is distinct from `TypeKind::Var(usize)` — the latter participates in unification and assignability, while TypeId is purely for propositional reasoning. Fresh-by-default biases toward correctness: a missed refinement opportunity is better than an incorrect one.

**Propositions.** The `Prop` enum encodes logical facts:

```rust
enum Prop {
    IsTrue(TypeId),
    RefinesTo(TypeId, Type),
    Not(Box<Prop>),
    Implies(Box<Prop>, Box<Prop>),
}
```

`RefinesTo(id, ty)` means "the type with the given ID can be replaced with `ty`". This decouples the proposition system from any specific type pattern — the operator that emits the proposition computes the refined type, and the derivation engine performs the substitution.

`Prop` implements `Eq + Hash` for use as a `HashMap` key. `RefinesTo` equality compares `(TypeId, Type::id)` pairs — it does not use structural type equality.

### Proposition Flow

Expression synthesis returns propositions alongside the type. The default behavior is to **forward all propositions** from sub-expressions upward. Callers decide whether and how to add them to child environments.

**Let bindings.** `let x = a; b` adds propositions from `a` to the child environment used to check `b`, so propositions accumulate naturally through sequential bindings.

**Type annotations.** An explicit annotation creates a fresh TypeId, forming a type boundary. Propositions from the initializer still flow to the enclosing scope but refer to the expression's own TypeIds, not the annotated binding's.

### Derivation Engine

When propositions are applied to a child `TypeEnv`, all consequences are derived eagerly via forward-chaining (modus ponens). Implications are indexed by their antecedent in a `HashMap<Prop, Vec<Prop>>`. When a new atomic proposition is proven:

1. Look up its consequents in the index.
2. Add each consequent to the proven set.
3. Recursively prove further consequences triggered by newly proven propositions.

The fully derived set — including a `RefinesTo` map — is stored in the `TypeEnv`.

### Refinement at Variable Resolution

Refinement is applied **at variable resolution time**, not when entering a scope. When a variable is looked up, the proven `RefinesTo` map is consulted and applied **recursively**: the substitution walks the type tree, replacing any type whose TypeId matches a proven refinement, then continues into the replacement (since it may contain further refinable TypeIds). This fixed-point application ensures nested refinements compose correctly.

This avoids the cost of walking all locals when entering a scope, handles intermediate TypeIds (like `?.` result wrappers) that never appear in a local's type directly, and ensures the LSP cursor tracking sees fully refined types.

### Storage and Lifetime Model

Propositions are stack-scoped. `TypeEnv` borrows propositions rather than owning them, fitting the existing `'a` lifetime pattern. Expression synthesis returns owned propositions on the caller's stack; child `TypeEnv`s borrow references to them. Derived propositions are owned by a derivation result struct on the stack, also borrowed by `TypeEnv`.

### Operators

**If-expression** — the primary consumer of propositions:

1. Check the condition, collecting its propositions.
2. Create child envs with those propositions applied plus:
   - `IsTrue(condition_type_id)` for the consequent branch.
   - `Not(IsTrue(condition_type_id))` for the else branch.
3. Propositions returned from both branches are wrapped in implications (branch assumption → branch proposition), preserving the logical relationship.

**`!` (NOT)** — emits a biconditional on `IsTrue`:
- `Implies(IsTrue(result), Not(IsTrue(operand)))`
- `Implies(Not(IsTrue(result)), IsTrue(operand))`

**`&&` (AND)** — emits conjunction implications:
- `Implies(IsTrue(result), IsTrue(lhs))` and `Implies(IsTrue(result), IsTrue(rhs))`
- RHS is checked in a child env where `IsTrue(lhs_id)` is assumed (matching short-circuit semantics). RHS propositions are wrapped in `Implies(IsTrue(lhs_id), ...)`.

**`||` (OR)** — emits disjunction implications:
- `Implies(Not(IsTrue(result)), Not(IsTrue(lhs)))` and `Implies(Not(IsTrue(result)), Not(IsTrue(rhs)))`
- RHS is checked in a child env where `Not(IsTrue(lhs_id))` is assumed. RHS propositions are wrapped in `Implies(Not(IsTrue(lhs_id)), ...)`.

**`!= nil` / `== nil`** — emit `RefinesTo` propositions with the unwrapped inner type:
- `x != nil` where `x : Optional(inner)`: `Implies(IsTrue(result), RefinesTo(optional_id, inner))`
- `x == nil`: `Implies(Not(IsTrue(result)), RefinesTo(optional_id, inner))`

**`?.` (optional chaining)** — creates a fresh `Optional` wrapper, reusing the inner type's TypeId. Emits:
- Source unwrap: `Implies(RefinesTo(result, inner), RefinesTo(source, unwrapped_source))`
- Field unwrap (only when the field is itself optional): `Implies(RefinesTo(result, inner), RefinesTo(field_type, field_inner))`

**`??` (nil coalesce)** — no propositions emitted. The result type propagates the inner TypeId from the optional.

### Example

```scl
let z: { x: Int }? = ...    // { x: Int }(1), Int(2), { x: Int }?(3)
let q = z?.x                 // Int?(4) wrapping Int(2)
                              // Implies(RefinesTo(4, Int(2)), RefinesTo(3, { x: Int }(1)))
let a = q != nil             // Bool(5)
                              // Implies(IsTrue(5), RefinesTo(4, Int(2)))
if (a)
  // Derivation: IsTrue(5) → RefinesTo(4, Int(2)) → RefinesTo(3, { x: Int }(1))
  // z is refined from { x: Int }? to { x: Int }
  z.x                        // : Int
```

### Scope

Emitters: `== nil`, `!= nil`, `!`, `&&`, `||`, `?.`. Consumer: `if`. Propositions do not cross function boundaries.

## Related Crates

- [DE](../de/) — compiles deployment configs using SCLC
- [CLI](../cli/) — provides REPL and local execution

For the SCL language reference, see [SCL Documentation](../../docs/scl/index.md).

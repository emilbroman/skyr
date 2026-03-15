# Syntax Reference

This document covers the syntax of SCL in detail.

## Comments

Line comments start with `//` and continue to the end of the line:

```scl
// This is a comment
let x = 42  // Inline comment
```

## Literals

### Integers

Integers are written as decimal digits. Negative integers use the unary minus operator:

```scl
0
42
-17
```

Leading zeros are not allowed except for `0` itself:

```scl
0     // Valid
007   // Invalid
```

### Floats

Floating-point numbers require digits on both sides of the decimal point:

```scl
3.14
0.5
-2.718
```

Invalid float syntax:

```scl
.5    // Invalid - must have digit before decimal
5.    // Invalid - must have digit after decimal
```

### Booleans

```scl
true
false
```

### Nil

The `nil` literal represents the absence of a value:

```scl
nil
```

### Strings

Strings are enclosed in double quotes:

```scl
"hello"
"hello, world"
""
```

#### Escape Sequences

| Sequence | Result |
|----------|--------|
| `\n` | Newline |
| `\r` | Carriage return |
| `\t` | Tab |
| `\\` | Backslash |
| `\{` | Literal `{` (prevents interpolation) |

```scl
"line one\nline two"
"tab\there"
"literal curly brace: \{not interpolated}"
```

#### String Interpolation

Expressions inside `{...}` are evaluated and converted to strings:

```scl
let name = "world"
"hello, {name}!"          // "hello, world!"

let x = 10
let y = 20
"sum: {x + y}"            // "sum: 30"

let config = { port: 8080 }
"port: {config.port}"     // "port: 8080"
```

### Lists

Lists are comma-separated values enclosed in brackets:

```scl
[1, 2, 3]
["a", "b", "c"]
[]                        // Empty list
[1, 2, 3,]                // Trailing comma is allowed
```

Lists can contain list comprehensions (see [List Comprehensions](#list-comprehensions)).

### Records

Records are comma-separated key-value pairs enclosed in braces:

```scl
{ name: "app", port: 8080 }
{ x: 1, y: 2 }
{}                        // Empty record
{ a: 1, b: 2, }           // Trailing comma is allowed
```

Field names are identifiers; values can be any expression.

When a field value is a variable with the same name as the field, you can use the shorthand syntax:

```scl
let name = "app"
let port = 8080
{ name, port }                // Same as { name: name, port: port }
{ name, port: 3000 }         // Shorthand and regular fields can be mixed
```

### Dicts

Dicts (dictionaries) are like records but with computed keys. They use `#{...}` syntax:

```scl
#{ "key": "value" }
#{ 1: "one", 2: "two" }
#{}                       // Empty dict
```

Unlike records, dict keys can be any expression:

```scl
let key = "dynamic"
#{ key: 42 }              // Key is the value of `key`, not the string "key"
```

## Operators

### Arithmetic Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `+` | Addition | `1 + 2` → `3` |
| `-` | Subtraction | `5 - 3` → `2` |
| `*` | Multiplication | `4 * 3` → `12` |
| `/` | Division | `10 / 3` → `3` |
| `-` (unary) | Negation | `-x` |

Addition also works on strings (concatenation):

```scl
"hello" + " " + "world"   // "hello world"
```

Arithmetic on mixed `Int` and `Float` produces `Float`:

```scl
1 + 2.0     // 3.0 (Float)
3.14 * 2    // 6.28 (Float)
```

### Comparison Operators

| Operator | Description |
|----------|-------------|
| `==` | Equal |
| `!=` | Not equal |
| `<` | Less than |
| `<=` | Less than or equal |
| `>` | Greater than |
| `>=` | Greater than or equal |

```scl
1 == 1      // true
"a" != "b"  // true
3 < 5       // true
```

### Logical Operators

| Operator | Description |
|----------|-------------|
| `&&` | Logical AND |
| `\|\|` | Logical OR |

```scl
true && false   // false
true || false   // true
```

### Type Cast Operator

The `as` operator casts an expression to a target type:

```scl
nil as Int?
value as Str
```

The `as` operator has the highest precedence, binding tighter than any binary operator. This means `1 + x as Int` casts only `x`, not `1 + x`. Use parentheses to cast a compound expression: `(1 + x) as Int`.

### Operator Precedence

From highest to lowest:

1. Postfix: `.property`, `(args)`, `as Type`
2. Unary: `-x`
3. Multiplicative: `*`, `/`
4. Additive: `+`, `-`
5. Comparison: `<`, `<=`, `>`, `>=`
6. Equality: `==`, `!=`
7. Logical AND: `&&`
8. Logical OR: `||`

All binary operators are left-associative.

## Expressions

### Variables

Variables are identifiers that refer to values:

```scl
name
count
myVariable
```

### Property Access

Access record or resource fields using dot notation:

```scl
config.port
user.name
pod.Container
```

### Function Calls

Functions are called with parenthesized, comma-separated arguments:

```scl
double(21)
add(1, 2)
Container.Image({ name: "app", context: "." })
```

### If Expressions

Conditional expressions with optional else clause:

```scl
if (condition) thenValue else elseValue

// Without else, the result is optional (may be nil)
if (condition) value
```

The condition must be in parentheses. Examples:

```scl
let status = if (enabled) "on" else "off"
let maybe = if (x > 0) x                    // Type is Int?
```

### Inline Let

Bind a value within an expression:

```scl
let x = 1; x + 1          // 2
```

The semicolon separates the binding from the body expression.

### Functions

Anonymous functions with typed parameters:

```scl
fn(x: Int) x * 2
fn(a: Int, b: Int) a + b
fn(config: { port: Int }) config.port
```

Functions capture their environment (closures):

```scl
let multiplier = 3
let times = fn(x: Int) x * multiplier
times(4)                  // 12
```

#### Generic Functions

Functions can declare type parameters in angle brackets before the parameter list:

```scl
fn<T>(value: T?) T                       // One type parameter
fn<T, U>(list: [T], f: fn(T) U) [U]     // Two type parameters
```

Type parameters can have upper bounds using `<:`:

```scl
fn<T <: { name: Str }>(item: T) item.name
```

This constrains `T` to types that have at least a `name: Str` field.

When calling generic functions, type arguments must be provided explicitly:

```scl
unwrap<Int>(maybeValue)
List.map<Int, Str>([1, 2], fn(x: Int) "{x}")
```

### Exceptions

#### Defining Exceptions

Define exception types with the `exception` keyword:

```scl
let MyError = exception
```

Exceptions can optionally carry a typed payload:

```scl
let ParseError = exception(Str)
```

#### Raising Exceptions

Use `raise` to throw an exception:

```scl
raise MyError
```

#### Try / Catch

Use `try`/`catch` to handle exceptions:

```scl
try riskyOperation()
    catch MyError: "default value"
```

Multiple catch clauses are allowed:

```scl
try riskyOperation()
    catch NotFound: "not found"
    catch ParseError(msg): "parse error: {msg}"
```

If the exception carries a payload, bind it with parentheses (`catch ParseError(msg): ...`). Without parentheses, the payload is ignored.

### List Comprehensions

Transform and filter lists within list literals:

```scl
// Map: apply expression to each element
[for (x in items) x * 2]

// Filter: include element conditionally
[for (x in items) if (x > 0) x]

// Conditional element (not iteration)
[1, if (condition) 2, 3]

// Nested comprehensions
[for (a in as) for (b in bs) a + b]

// Complex combinations
[for (x in xs) if (x > 0) for (y in ys) x * y]
```

The `for` keyword iterates; `if` filters. Multiple `for` and `if` clauses can be stacked.

## Statements

### Let Bindings

Bind a value to a name at module scope:

```scl
let x = 42
let config = { port: 8080 }
```

### Export

Export a binding for use by other modules:

```scl
export let config = { port: 8080 }
```

### Import

Import a module:

```scl
import Std/Container
import Std/Encoding
```

After importing, access the module's exports via the module name:

```scl
Container.Image(...)
Encoding.toJson(...)
```

### Type Declarations

Declare a named type alias at module scope:

```scl
type Port Int
type Config { host: Str, port: Int }
```

Type declarations can be exported:

```scl
export type Config { host: Str, port: Int }
```

Generic type declarations use type parameters:

```scl
export type Result<T> { ok: T?, error: Str? }
```

Generic types are applied with angle brackets at usage sites:

```scl
type Pair<A, B> { fst: A, snd: B }
let p: Pair<Int, Str> = { fst: 1, snd: "hello" }
```

Type names can be accessed from imported modules using dot notation (type-level property access):

```scl
import MyLib
let cfg: MyLib.Config = { host: "localhost", port: 8080 }
```

Type declarations and value bindings live in separate namespaces, so a name can refer to both a type and a value:

```scl
export type Config { host: Str, port: Int }
export let Config = fn(host: Str, port: Int) { host: host, port: port }
```

### Expression Statements

Expressions can appear at module scope, typically for resources:

```scl
import Std/Artifact

Artifact.File({ name: "test.txt", contents: "hello" })
```

## Type Annotations

Type annotations appear after colons in function parameters, after `extern` declarations, and in `type` declarations.

### Basic Types

```scl
Int
Float
Str
Bool
Any
```

### Optional Types

Append `?` to make a type optional (can be `nil`):

```scl
Str?
Int?
```

### List Types

Wrap in brackets:

```scl
[Int]
[Str]
[[Int]]                   // List of lists
```

### Record Types

```scl
{ name: Str, port: Int }
{ config: { nested: Bool } }
```

### Dict Types

```scl
#{ Str: Int }
#{ Int: Str }
```

### Function Types

```scl
fn(Int) Int
fn(Int, Int) Int
fn({ name: Str }) { result: Int }
```

Generic function types use angle brackets with optional bounds:

```scl
fn<T>(T) T
fn<T, U>(T, fn(T) U) [U]
fn<T <: { name: Str }>(T) Str
```

### Type-Level Property Access

Access exported type declarations from imported modules using dot notation:

```scl
import MyLib
fn(cfg: MyLib.Config) cfg.host
```

This works anywhere a type expression is expected.

## Extern Declarations

Declare external (built-in) functions:

```scl
export let toJson = extern "Std/Encoding.toJson": fn(Any) Str
```

The string after `extern` is the internal function name. This is primarily used in standard library modules.

## Module Structure

A typical module:

```scl
// Imports first
import Std/Container
import Std/Artifact

// Bindings and resources
let image = Container.Image({
    name: "my-app",
    context: ".",
    containerfile: "Containerfile",
})

let pod = Container.Pod({ name: "my-app" })

// Expression statements for side effects
pod.Container({
    name: "app",
    image: image.fullname,
})

// Exports (typically in library modules)
export let config = { ... }
```

The entry point for a Skyr repository is always `Main.scl`.

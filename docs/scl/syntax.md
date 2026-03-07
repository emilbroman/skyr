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

### Operator Precedence

From highest to lowest:

1. Unary: `-x`
2. Multiplicative: `*`, `/`
3. Additive: `+`, `-`
4. Comparison: `<`, `<=`, `>`, `>=`
5. Equality: `==`, `!=`
6. Logical AND: `&&`
7. Logical OR: `||`

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

### Expression Statements

Expressions can appear at module scope, typically for resources:

```scl
import Std/Artifact

Artifact.File({ name: "test.txt", contents: "hello" })
```

## Type Annotations

Type annotations appear after colons in function parameters and after `extern` declarations.

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

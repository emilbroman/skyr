# Type System

SCL is statically typed with type inference. Most of the time you don't need to write type annotations—the compiler figures out the types for you.

## Basic Types

### Int

64-bit signed integers:

```scl
let count = 42
let negative = -17
```

### Float

64-bit floating-point numbers:

```scl
let pi = 3.14159
let ratio = 0.5
```

### Str

Unicode strings:

```scl
let name = "hello"
let greeting = "Hello, {name}!"
```

### Bool

Boolean values:

```scl
let enabled = true
let disabled = false
```

### Any

The dynamic type. Values of type `Any` bypass static type checking:

```scl
let data: Any = Encoding.fromJson(jsonStr)
```

Use `Any` sparingly—it disables type safety for that value.

## Composite Types

### Optional Types

Optional types can hold a value or `nil`. Written as `Type?`:

```scl
let maybePort: Int? = nil
let definitelyPort: Int? = 8080
```

Non-optional values are automatically compatible with optional types:

```scl
let x: Int = 42
let y: Int? = x   // OK: Int is assignable to Int?
```

But not the other way:

```scl
let x: Int? = 42
let y: Int = x    // Error: Int? is not assignable to Int
```

### Lists

Ordered sequences of values with the same type. Written as `[Type]`:

```scl
let numbers: [Int] = [1, 2, 3]
let names: [Str] = ["alice", "bob"]
let nested: [[Int]] = [[1, 2], [3, 4]]
let empty: [Int] = []
```

Type inference works on lists:

```scl
let items = [1, 2, 3]    // Inferred as [Int]
```

Empty lists infer the special bottom type `[Never]`, which is assignable to any list type.

Elements can be accessed by index using `list[index]`, which returns an `Optional` since the index may be out of bounds:

```scl
let items = [10, 20, 30]
items[1]    // 20 (Optional<Int>)
items[99]   // nil (Optional<Int>)
```

### Records

Named fields with specific types. Written as `{ field: Type, ... }`:

```scl
let config: { port: Int, host: Str } = {
    port: 8080,
    host: "localhost",
}
```

Records support two kinds of subtyping:

**Width subtyping**: A record with more fields is assignable to a record type with fewer fields:

```scl
let full = { a: 1, b: "two", c: true }
let partial: { a: Int, c: Bool } = full   // OK: extra field 'b' is ignored
```

**Depth subtyping**: Field types can be subtypes:

```scl
let strict = { value: 42 }
let flexible: { value: Int? } = strict    // OK: Int is subtype of Int?
```

### Dicts

Key-value maps with homogeneous types. Written as `#{ KeyType: ValueType }`:

```scl
let lookup: #{ Str: Int } = #{
    "one": 1,
    "two": 2,
}
```

Dicts are covariant in both key and value types:

```scl
let strict: #{ Str: Int } = #{ "a": 1 }
let flexible: #{ Str?: Int? } = strict    // OK
```

Values can be looked up by key using indexed access (`dict[key]`), which returns an `Optional` since the key may not be present:

```scl
let lookup = #{ "a": 1, "b": 2 }
lookup["a"]   // 1 (Optional<Int>)
lookup["z"]   // nil (Optional<Int>)
```

### Functions

Function types specify parameter and return types. Written as `fn(Param, ...) Return`:

```scl
let double: fn(Int) Int = fn(x: Int) x * 2
let add: fn(Int, Int) Int = fn(a: Int, b: Int) a + b
```

Functions with record parameters:

```scl
let configure: fn({ port: Int }) Str = fn(config: { port: Int })
    "Configured on port {config.port}"
```

### Generic Functions

Functions can be parameterized over types using type parameters in angle brackets:

```scl
let identity = fn<T>(x: T) x
let first = fn<T, U>(a: T, b: U) a
```

Type parameters can have upper bounds with `<:`, constraining them to subtypes:

```scl
let getName = fn<T <: { name: Str }>(item: T) item.name
getName({ name: "alice", age: 30 })   // "alice"
```

At call sites, type arguments must be provided explicitly (e.g., `List.map<Int, Str>(...)`). Generic functions enable reusable utilities like `List.map` and `Option.unwrap` that work with any type.

### Exception Types

Exception types are defined with the `exception` keyword and can optionally carry a payload:

```scl
let NotFound = exception           // No payload
let ParseError = exception(Str)    // Carries a Str payload
```

Exceptions are raised with `raise` and caught with `try`/`catch` (see [Syntax Reference](syntax.md#exceptions)).

## Type Declarations

Name a type for reuse with the `type` keyword:

```scl
type Port Int
type Config { host: Str, port: Int }
```

Type declarations can be exported and accessed from other modules:

```scl
// In MyLib.scl
export type Config { host: Str, port: Int }

// In Main.scl
import MyLib
let cfg: MyLib.Config = { host: "localhost", port: 8080 }
```

Generic type declarations accept type parameters, applied at usage sites with angle brackets:

```scl
type Result<T> { ok: T?, error: Str? }
let r: Result<Int> = { ok: 42, error: nil }
```

Type names and value names are in separate namespaces, so a module can export both `type Config` and `let Config` without conflict.

## Type Inference

SCL infers types from values and context. You rarely need explicit annotations.

### From Literals

```scl
let x = 42           // Int
let y = 3.14         // Float
let s = "hello"      // Str
let b = true         // Bool
let n = nil          // Never?, only compatible with optional types
```

### From Context

When the expected type is known, SCL uses it to check and infer:

```scl
let config: { port: Int, debug: Bool } = {
    port: 8080,      // Checked as Int
    debug: true,     // Checked as Bool
}
```

### From First Element

Lists and dicts infer their element types from the first entry:

```scl
let items = [1, 2, 3]              // Inferred: [Int]
let lookup = #{ "a": 1, "b": 2 }   // Inferred: #{ Str: Int }
```

## Type Annotations

Type annotations are required in function parameters:

```scl
fn(x: Int) x * 2
fn(config: { port: Int }) config.port
```

They're optional elsewhere but can be used for documentation or to constrain types:

```scl
let port: Int = 8080
let maybeHost: Str? = nil
```

## Special Types

### Never

The bottom type, representing values that cannot exist. An empty list has type `[Never]`, which is assignable to any list type:

```scl
let empty: [Int] = []      // [] has type [Never], assignable to [Int]
let strings: [Str] = []    // Also works
```

### Never?

The type of `nil`. Only assignable to optional types:

```scl
let nothing = nil          // Type is Never?
let maybe: Int? = nil      // OK: Never? is assignable to Int?
let required: Int = nil    // Error: Never? is not assignable to Int
```

## Type Compatibility

SCL uses structural typing. Two types are compatible if they have the same shape.

### Records are Structural

```scl
// These are the same type:
let a: { x: Int, y: Int } = { x: 1, y: 2 }
let b: { y: Int, x: Int } = a   // Field order doesn't matter
```

### Subtyping Rules

1. `T` is assignable to `T?` (non-optional to optional)
2. `Never` is assignable to any type
3. Any type is assignable to `Any`
4. Records: `{ a: T1, b: T2 }` is assignable to `{ a: T1 }` (width subtyping)
5. Records: `{ a: T }` is assignable to `{ a: U }` if `T` is assignable to `U` (depth subtyping)
6. Lists: `[T]` is assignable to `[U]` if `T` is assignable to `U` (covariant)
7. Dicts: `#{ K1: V1 }` is assignable to `#{ K2: V2 }` if both `K1` to `K2` and `V1` to `V2` (covariant)

## Arithmetic Type Coercion

When mixing `Int` and `Float` in arithmetic, the result is `Float`:

```scl
1 + 2        // Int
1.0 + 2.0    // Float
1 + 2.0      // Float (Int is coerced to Float)
3.14 * 2     // Float
```

## Type Errors

The compiler reports type mismatches with helpful messages:

```scl
let x: Int = "hello"
// Error: Str is not assignable to Int

let config: { port: Int } = { port: "8080" }
// Error: Str is not assignable to Int (in field 'port')
```

Errors include causal chains for nested type mismatches, helping you trace the source of the problem.

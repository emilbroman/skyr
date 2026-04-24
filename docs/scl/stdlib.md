# Standard Library Reference

## Std/DNS

DNS resource management.

### DNS.ARecord

Create a DNS A record.

```scl
import Std/DNS
import Std/Time

DNS.ARecord({
    name: "example.com",
    ttl: Time.minute,
    addresses: ["93.184.216.34"],
})
```

| | Fields |
|---|--------|
| **Inputs** | `name: Str` — fully-qualified domain name |
| | `ttl: Time.Duration` — time to live |
| | `addresses: [Str]` — list of IPv4 addresses |
| **Outputs** | Same as inputs |

## Std/Package

Types describing a repository's cross-repo dependency manifest. Used in `Package.scle` files at the root of a repository — see [Cross-Repo Imports](../cross-repo-imports.md).

### Package.Manifest

```scl
export type Manifest {
    dependencies: #{ Str: Str }
}
```

A manifest declares the foreign repositories this repo depends on. Each `dependencies` entry maps `Org/Repo` to a Git-ref-like specifier:

- A bare branch name, e.g. `"main"`.
- A tag, prefixed with `tag:`, e.g. `"tag:v1.2.0"`.
- A 40-character hex commit hash for a deterministic pin.

## Std/Str

Pure string manipulation functions. All functions operate on Unicode scalar values (codepoints).

```scl
import Std/Str
```

### Str.length

Returns the number of Unicode scalar values (codepoints) in a string.

```scl
Str.length("hello")   // 5
Str.length("日本語")  // 3
```

### Str.isEmpty

Returns `true` if the string has zero length.

```scl
Str.isEmpty("")   // true
Str.isEmpty("x")  // false
```

### Str.toUpper / Str.toLower

Convert a string to uppercase or lowercase using full Unicode case mapping.

```scl
Str.toUpper("hello")  // "HELLO"
Str.toLower("WORLD")  // "world"
```

### Str.trim / Str.trimStart / Str.trimEnd

Strip leading and/or trailing Unicode whitespace.

```scl
Str.trim("  hello  ")       // "hello"
Str.trimStart("  hello  ")  // "hello  "
Str.trimEnd("  hello  ")    // "  hello"
```

### Str.split

Split a string on a literal separator. Raises an error if `separator` is empty.

```scl
Str.split("a,b,,c", ",")   // ["a", "b", "", "c"]
Str.split("a::b", "::")    // ["a", "b"]
```

### Str.join

Concatenate a list of strings with a separator between each element.

```scl
Str.join(["a", "b", "c"], ",")  // "a,b,c"
Str.join(["a", "b"], "")        // "ab"
```

### Str.contains / Str.startsWith / Str.endsWith

Test whether a string contains, starts with, or ends with a substring.

```scl
Str.contains("hello world", "world")   // true
Str.startsWith("hello", "he")          // true
Str.endsWith("hello", "lo")            // true
```

### Str.replace

Replace every occurrence of `from` with `to`. Raises an error if `from` is empty.

```scl
Str.replace("foo bar foo", "foo", "baz")  // "baz bar baz"
```

### Str.slice

Return a substring by Unicode scalar index. Pass `nil` for `end` to slice to the end. Indices are clamped to `[0, length]`; if `end <= start`, returns `""`. Raises an error if either index is negative.

```scl
Str.slice("hello", 1, 4)    // "ell"
Str.slice("hello", 2, nil)  // "llo"
Str.slice("hello", 0, 100)  // "hello"
```

### Str.indexOf

Return the scalar index of the first occurrence of `needle`, or `nil` if not found. An empty needle returns `0`.

```scl
Str.indexOf("hello", "ll")  // 2
Str.indexOf("hello", "x")   // nil
Str.indexOf("hello", "")    // 0
```

### Str.repeat

Repeat a string a given number of times. Raises an error if `times` is negative.

```scl
Str.repeat("ab", 3)  // "ababab"
Str.repeat("x", 0)   // ""
```

### Str.padStart / Str.padEnd

Pad a string to `width` Unicode scalars with `fill` (default `" "`). If the string is already at least `width` long, it is returned unchanged. Raises an error if `fill` is empty or `width` is negative.

```scl
Str.padStart("7", 3, "0")    // "007"
Str.padEnd("7", 3, "0")      // "700"
Str.padStart("12", 5, nil)   // "   12"
Str.padStart("x", 5, "ab")   // "ababx"
```

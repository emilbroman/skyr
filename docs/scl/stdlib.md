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
| **Outputs** | `fqdn: Str` — fully-qualified domain name (name + zone) |
| | `ttl: Time.Duration` |
| | `addresses: [Str]` |

### DNS.AAAARecord

Create a DNS AAAA record (IPv6).

```scl
import Std/DNS
import Std/Time

DNS.AAAARecord({
    name: "example.com",
    ttl: Time.minute,
    addresses: ["2001:db8::1"],
})
```

| | Fields |
|---|--------|
| **Inputs** | `name: Str` — fully-qualified domain name |
| | `ttl: Time.Duration` — time to live |
| | `addresses: [Str]` — list of IPv6 addresses |
| **Outputs** | `fqdn: Str` — fully-qualified domain name (name + zone) |
| | `ttl: Time.Duration` |
| | `addresses: [Str]` |

### DNS.CNAMERecord

Create a DNS CNAME record.

```scl
import Std/DNS
import Std/Time

DNS.CNAMERecord({
    name: "alias.example.com",
    ttl: Time.minute,
    target: "canonical.example.com",
})
```

| | Fields |
|---|--------|
| **Inputs** | `name: Str` — fully-qualified domain name |
| | `ttl: Time.Duration` — time to live |
| | `target: Str` — canonical name target |
| **Outputs** | `fqdn: Str` — fully-qualified domain name (name + zone) |
| | `ttl: Time.Duration` |
| | `target: Str` |

### DNS.TXTRecord

Create a DNS TXT record.

```scl
import Std/DNS
import Std/Time

DNS.TXTRecord({
    name: "example.com",
    ttl: Time.minute,
    values: ["v=spf1 -all"],
})
```

| | Fields |
|---|--------|
| **Inputs** | `name: Str` — fully-qualified domain name |
| | `ttl: Time.Duration` — time to live |
| | `values: [Str]` — one or more text strings |
| **Outputs** | `fqdn: Str` — fully-qualified domain name (name + zone) |
| | `ttl: Time.Duration` |
| | `values: [Str]` |

### DNS.MXRecord

Create a DNS MX record.

```scl
import Std/DNS
import Std/Time

DNS.MXRecord({
    name: "example.com",
    ttl: Time.minute,
    exchanges: [{ priority: 10, host: "mail.example.com" }],
})
```

| | Fields |
|---|--------|
| **Inputs** | `name: Str` — fully-qualified domain name |
| | `ttl: Time.Duration` — time to live |
| | `exchanges: [{priority: Int, host: Str}]` — mail exchangers |
| **Outputs** | `fqdn: Str` — fully-qualified domain name (name + zone) |
| | `ttl: Time.Duration` |
| | `exchanges: [{priority: Int, host: Str}]` |

### DNS.SRVRecord

Create a DNS SRV record.

```scl
import Std/DNS
import Std/Time

DNS.SRVRecord({
    name: "_svc._tcp.example.com",
    ttl: Time.minute,
    records: [{ priority: 10, weight: 5, port: 443, target: "svc.example.com" }],
})
```

| | Fields |
|---|--------|
| **Inputs** | `name: Str` — fully-qualified domain name |
| | `ttl: Time.Duration` — time to live |
| | `records: [{priority: Int, weight: Int, port: Int, target: Str}]` — service locations |
| **Outputs** | `fqdn: Str` — fully-qualified domain name (name + zone) |
| | `ttl: Time.Duration` |
| | `records: [{priority: Int, weight: Int, port: Int, target: Str}]` |

## Std/Crypto

Cryptographic operations: key generation, certificate management, and hashing.

### Hashing

All hash functions accept a `Str` and return a lowercase hex-encoded digest string. The input is hashed as its UTF-8 bytes.

#### Crypto.sha1

```
sha1: fn(Str) Str
```

Compute the SHA-1 hash of the input. Returns a 40-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.sha1("hello")  // "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
```

#### Crypto.sha256

```
sha256: fn(Str) Str
```

Compute the SHA-256 hash of the input. Returns a 64-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.sha256("hello")  // "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
```

#### Crypto.sha512

```
sha512: fn(Str) Str
```

Compute the SHA-512 hash of the input. Returns a 128-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.sha512("hello")  // "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043"
```

#### Crypto.md5

```
md5: fn(Str) Str
```

Compute the MD5 hash of the input. Returns a 32-character lowercase hex digest.

**Insecure — do not use for authentication or integrity checking. Provided for legacy compatibility only.**

```scl
import Std/Crypto

let digest = Crypto.md5("hello")  // "5d41402abc4b2a76b9719d911017c592"
```

#### Crypto.hmacSha256

```
hmacSha256: fn(Str, Str) Str
```

Compute the HMAC-SHA-256 of a message using a key. The first argument is the key, the second is the message. Both are interpreted as UTF-8 bytes. Returns a 64-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.hmacSha256("key", "The quick brown fox jumps over the lazy dog")
// "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
```

#### Crypto.hmacSha512

```
hmacSha512: fn(Str, Str) Str
```

Compute the HMAC-SHA-512 of a message using a key. The first argument is the key, the second is the message. Both are interpreted as UTF-8 bytes. Returns a 128-character lowercase hex digest.

```scl
import Std/Crypto

let digest = Crypto.hmacSha512("key", "The quick brown fox jumps over the lazy dog")
// "b42af09057bac1e2d41708e48a902e09b5ff7f12ab428a4fe86653c73dd248fb82f948a549f7b791a5b41915ee4d1ec3935357e4e2317250d0372afa2ebeeb3a"
```

## Std/HTTP

HTTP requests as resources.

### HTTP.Get

Perform an HTTP GET request.

```scl
import Std/HTTP

let probe = HTTP.Get({
    url: "https://example.com",
    headers: #{ "Accept": "text/html" },
})
```

| | Fields |
|---|--------|
| **Inputs** | `url: Str` — URL to fetch |
| | `headers: #{Str: Str}?` — Request headers to send (defaults to empty) |
| **Outputs** | `url: Str` |
| | `headers: #{Str: Str}` — Response headers, with names lowercased |
| | `status: Int` — HTTP response status code |
| | `body: Str` — Response body |

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

## Std/List

List manipulation functions covering creation, transformation, filtering, searching, and structural operations.

```scl
import Std/List
```

### List.range

Generate a list of integers from `0` up to (but not including) `n`. Returns `[]` when `n` is zero. Raises an error if `n` is negative.

```scl
List.range(5)  // [0, 1, 2, 3, 4]
List.range(0)  // []
```

### List.map

Apply a function to each element, returning a new list of transformed values.

```scl
List.map([1, 2, 3], fn(x: Int) x * 2)  // [2, 4, 6]
```

### List.append

Add an element to the end of a list.

```scl
List.append([1, 2], 3)  // [1, 2, 3]
```

### List.prepend

Add an element to the front of a list.

```scl
List.prepend([2, 3], 1)  // [1, 2, 3]
```

### List.concat

Flatten a list of lists into a single list.

```scl
List.concat([[1, 2], [3], [4, 5]])  // [1, 2, 3, 4, 5]
```

### List.filter

Keep only elements that satisfy a predicate.

```scl
List.filter([1, 2, 3, 4], fn(x: Int) x > 2)  // [3, 4]
```

### List.flatMap

Map each element to a list, then flatten the results into a single list.

```scl
List.flatMap([1, 2, 3], fn(x: Int) [x, x * 10])  // [1, 10, 2, 20, 3, 30]
```

### List.length

Returns the number of elements in the list.

```scl
List.length([1, 2, 3])  // 3
List.length<Int>([])    // 0
```

### List.isEmpty

Returns `true` if the list has zero elements.

```scl
List.isEmpty<Int>([])  // true
List.isEmpty([1, 2])   // false
```

### List.reverse

Return a new list with the elements in reverse order.

```scl
List.reverse([1, 2, 3])  // [3, 2, 1]
```

### List.first / List.last

Return the first or last element, or `nil` if the list is empty.

```scl
List.first([1, 2, 3])    // 1
List.last([1, 2, 3])     // 3
List.first<Int>([])      // nil
```

### List.slice

Return a sublist by index. Pass `nil` for `end` to slice to the end. Indices are clamped to `[0, length]`; if `end <= start`, returns `[]`. Raises an error if either index is negative.

```scl
List.slice([1, 2, 3, 4, 5], 1, 4)   // [2, 3, 4]
List.slice([1, 2, 3], 2, nil)        // [3]
List.slice([1, 2, 3], 0, 100)        // [1, 2, 3]
```

### List.take / List.skip

`take` returns the first `n` elements (or fewer if the list is shorter). `skip` returns all but the first `n` elements. Both raise an error if `n` is negative.

```scl
List.take([1, 2, 3, 4, 5], 3)  // [1, 2, 3]
List.skip([1, 2, 3, 4, 5], 2)  // [3, 4, 5]
```

### List.contains

Returns `true` if the needle occurs anywhere in the list. Uses value equality.

```scl
List.contains([1, 2, 3], 2)  // true
List.contains([1, 2, 3], 5)  // false
```

### List.indexOf

Returns the index of the first occurrence of the needle, or `nil` if not found. Uses value equality.

```scl
List.indexOf([10, 20, 30], 20)     // 1
List.indexOf<Int>([10, 20, 30], 99)  // nil
```

### List.any / List.all

`any` returns `true` if at least one element satisfies the predicate. `all` returns `true` if every element does (vacuously `true` for an empty list).

```scl
List.any([1, 2, 3], fn(x: Int) x > 2)    // true
List.all([2, 4, 6], fn(x: Int) x > 1)    // true
List.all<Int>([], fn(x: Int) false)      // true
```

### List.find

Return the first element matching the predicate, or `nil` if none match.

```scl
List.find([1, 2, 3, 4], fn(x: Int) x > 2)   // 3
List.find<Int>([1, 2, 3], fn(x: Int) x > 10)  // nil
```

### List.zip

Pair up two lists element-wise into records `{a, b}`. The result length is the shorter of the two inputs.

```scl
List.zip([1, 2], ["x", "y"])  // [{a: 1, b: "x"}, {a: 2, b: "y"}]
```

### List.distinct

Remove duplicate elements, preserving the order of first occurrence. Uses value equality.

```scl
List.distinct([1, 2, 1, 3, 2, 4])  // [1, 2, 3, 4]
```

## Std/Dict

Dict manipulation functions for `#{K: V}` values — the type used for environment variables, labels, and other key-value configuration.

```scl
import Std/Dict
```

### Dict.size

Returns the number of entries in the dict.

```scl
Dict.size<Str, Int>(#{})           // 0
Dict.size(#{"a": 1, "b": 2})      // 2
```

### Dict.isEmpty

Returns `true` if the dict has zero entries.

```scl
Dict.isEmpty<Str, Int>(#{})   // true
Dict.isEmpty(#{"a": 1})       // false
```

### Dict.keys

Returns all keys in insertion order.

```scl
Dict.keys(#{"a": 1, "b": 2})  // ["a", "b"]
```

### Dict.values

Returns all values in insertion order.

```scl
Dict.values(#{"a": 1, "b": 2})  // [1, 2]
```

### Dict.entries

Returns all entries as `{key, value}` records in insertion order.

```scl
Dict.entries(#{"a": 1})  // [{key: "a", value: 1}]
```

### Dict.has

Returns `true` if the dict contains an entry for `key`. Uses value equality.

```scl
Dict.has(#{"x": 1, "y": 2}, "x")  // true
Dict.has(#{"x": 1, "y": 2}, "z")  // false
```

### Dict.get

Returns the value for `key`, or `nil` if absent.

```scl
Dict.get(#{"x": 1, "y": 2}, "x")  // 1
Dict.get(#{"x": 1, "y": 2}, "z")  // nil
```

### Dict.insert

Returns a new dict with `key` mapped to `value`. If `key` already exists, the entry is updated in place (preserving its insertion-order position); otherwise the entry is appended.

```scl
Dict.insert(#{"x": 1}, "y", 2)         // #{"x": 1, "y": 2}
Dict.insert(#{"x": 1, "y": 2}, "x", 9) // #{"x": 9, "y": 2}
```

### Dict.remove

Returns a new dict without `key`. No-op if `key` is absent.

```scl
Dict.remove(#{"x": 1, "y": 2}, "x")  // #{"y": 2}
Dict.remove(#{"x": 1}, "missing")     // #{"x": 1}
```

### Dict.merge

Merges two dicts, with right-side entries winning on key conflicts. Left-side ordering is preserved for keys that appear in both.

```scl
Dict.merge(#{"a": 1, "b": 2}, #{"b": 99, "c": 3})
// #{"a": 1, "b": 99, "c": 3}
```

### Dict.fromList

Builds a dict from a list of `{key, value}` records. On duplicate keys, the last occurrence wins.

```scl
Dict.fromList<Str, Int>([{key: "a", value: 1}, {key: "b", value: 2}])
// #{"a": 1, "b": 2}
```

### Dict.map

Applies a function to each value, returning a new dict with the same keys and transformed values.

```scl
Dict.map(#{"a": 1, "b": 2}, fn(k: Str, v: Int) v * 10)
// #{"a": 10, "b": 20}
```

### Dict.filter

Keeps only entries that satisfy a predicate.

```scl
Dict.filter(#{"a": 1, "b": 2, "c": 3}, fn(k: Str, v: Int) v > 1)
// #{"b": 2, "c": 3}
```

## Std/Path

Functions for inspecting and composing repo-rooted `Path` values.

A `Path` value is anchored to the package it was originally resolved against, and carries a content hash identifying the file or directory at that path within that package's commit. Manipulation functions that produce new `Path` values (`Path.join`, `Path.parent`, `Path.fromStr`) look up the content hash of the resulting path against the input's anchor package, keeping manipulated paths usable as content-addressed references (see [`Std/Container.Pod`](#stdcontainer)). When the resulting path does not exist in that package, the function raises `Path.NotFound(path)` rather than silently returning a path with a null hash.

```scl
import Std/Path
```

### Path.NotFound

Exception raised by `Path.join`, `Path.parent`, and `Path.fromStr` when the manipulated path is syntactically valid but does not refer to an existing entry in the input path's anchor package. The payload is the manipulated path string.

```scl
let NotFound = Path.NotFound

try
    Path.join(/src, "missing")
catch NotFound(p):
    "couldn't resolve {p}"
```

### Path.join

Append a string segment to a path. The segment may contain `/` separators; `.` and `..` are normalized away. A leading `/` in the segment is treated as a separator (relative). The result is anchored to the same package as the input path. Raises `Path.NotFound` when the joined path does not exist in that package.

```scl
Path.join(/foo, "bar")       // /foo/bar
Path.join(/foo/bar, "..")    // /foo
Path.join(/foo, "bar/baz")   // /foo/bar/baz
```

### Path.parent

Return the parent directory of the path, or `nil` at the root. The result is anchored to the same package as the input path. Raises `Path.NotFound` when the parent path does not exist in that package.

```scl
Path.parent(/foo/bar)  // /foo
Path.parent(/foo)      // /
Path.parent(/)         // nil
```

### Path.basename

Return the final segment of the path. Returns `""` at the root.

```scl
Path.basename(/foo/bar.txt)  // "bar.txt"
Path.basename(/foo)          // "foo"
Path.basename(/)             // ""
```

### Path.extname

Return the file extension including the leading dot (e.g. `".txt"`), or `nil` if none. A leading dot in a dotfile does not count as an extension.

```scl
Path.extname(/foo/bar.txt)        // ".txt"
Path.extname(/foo/bar)            // nil
Path.extname(/foo/.bashrc)        // nil
Path.extname(/foo/archive.tar.gz) // ".gz"
```

### Path.stem

Return the basename without its extension. For dotfiles, the whole basename is returned.

```scl
Path.stem(/foo/bar.txt)        // "bar"
Path.stem(/foo/bar)            // "bar"
Path.stem(/foo/.bashrc)        // ".bashrc"
Path.stem(/foo/archive.tar.gz) // "archive.tar"
```

### Path.segments

Return the path components in order. Returns `[]` for the root.

```scl
Path.segments(/foo/bar/baz)  // ["foo", "bar", "baz"]
Path.segments(/)             // []
```

### Path.isRoot

Return `true` if the path is the root `/`.

```scl
Path.isRoot(/)     // true
Path.isRoot(/foo)  // false
```

### Path.toStr

Return the canonical string form of the path (always begins with `/`).

```scl
Path.toStr(/foo/bar)  // "/foo/bar"
Path.toStr(/)         // "/"
```

### Path.fromStr

Parse a string as an absolute, repo-rooted path, anchored to the same package as `anchor`. The anchor is only used for its package — pass `/` from the calling module for the conventional case. Returns `nil` on syntactically invalid input. Raises `Path.NotFound` when the string is a valid absolute path but does not exist in `anchor`'s package. The string must start with `/`; components are normalized (`.` dropped, `..` pops a segment, empty segments from doubled slashes are dropped). A `..` that would escape the root makes the input invalid.

```scl
Path.fromStr(/, "/foo/bar")          // /foo/bar
Path.fromStr(/, "/foo/./bar/../baz") // /foo/baz
Path.fromStr(/, "foo")               // nil
Path.fromStr(/, "/..")               // nil
Path.fromStr(/, "")                  // nil
```

## Std/Encoding

Encoding and decoding functions for JSON, YAML, TOML, base64, hex, and URL formats. All functions treating binary data as UTF-8 strings; decoding functions raise a runtime error when the decoded bytes are not valid UTF-8.

```scl
import Std/Encoding
```

### Encoding.toJson

Serialize any value to a JSON string.

Value mappings:
- `Int` / `Float` → JSON number
- `Str` → JSON string
- `Bool` → JSON `true` / `false`
- `nil` → JSON `null`
- `List` → JSON array
- `Record` / `Dict` → JSON object (non-string dict keys are stringified)

Functions and exceptions cannot be serialized and will raise a runtime error.

```scl
Encoding.toJson(42)              // "42"
Encoding.toJson("hello")         // "\"hello\""
Encoding.toJson({ key: "val" })  // "{\"key\":\"val\"}"
```

### Encoding.fromJson

Parse a JSON string into a value.

Value mappings:
- JSON number → `Float`
- JSON string → `Str`
- JSON `true` / `false` → `Bool`
- JSON `null` → `nil`
- JSON array → `List`
- JSON object → `Dict` (with `Str` keys)

Note: JSON numbers always become `Float`, not `Int`.

```scl
Encoding.fromJson("42")            // 42.0
Encoding.fromJson("{\"a\": 1}")    // #{"a": 1.0}
```

### Encoding.toBase64

Encode a string as standard base64 (padded, standard alphabet). The input is treated as UTF-8 bytes.

```scl
Encoding.toBase64("hello")  // "aGVsbG8="
Encoding.toBase64("")       // ""
```

### Encoding.fromBase64

Decode a standard base64 string back to a UTF-8 string. Raises if the input is not valid base64 or the decoded bytes are not valid UTF-8.

```scl
Encoding.fromBase64("aGVsbG8=")  // "hello"
```

### Encoding.toBase64Url

Encode a string as URL-safe base64 (unpadded, URL-safe alphabet). Uses `-` and `_` instead of `+` and `/`, and omits `=` padding.

```scl
Encoding.toBase64Url("hello")  // "aGVsbG8"
```

### Encoding.fromBase64Url

Decode a URL-safe base64 string back to a UTF-8 string. Accepts both padded and unpadded input. Raises if the input is malformed or decoded bytes are not valid UTF-8.

```scl
Encoding.fromBase64Url("aGVsbG8")   // "hello"
Encoding.fromBase64Url("aGVsbG8=")  // "hello"
```

### Encoding.toHex

Encode a string as lowercase hexadecimal. Each byte of the UTF-8 input becomes two hex digits.

```scl
Encoding.toHex("hi")  // "6869"
Encoding.toHex("")    // ""
```

### Encoding.fromHex

Decode a hexadecimal string back to a UTF-8 string. Accepts both uppercase and lowercase hex digits. Raises if the input has an odd number of characters, contains non-hex characters, or decoded bytes are not valid UTF-8.

```scl
Encoding.fromHex("6869")  // "hi"
Encoding.fromHex("4869")  // "Hi"
```

### Encoding.toYaml

Serialize any value to a YAML string. Value mappings follow the same conventions as `toJson`.

```scl
Encoding.toYaml({ name: "test", count: 42 })
// "count: 42\nname: test\n"
```

### Encoding.fromYaml

Parse a YAML string into a value.

Value mappings:
- YAML integer → `Int`
- YAML float → `Float`
- YAML bool → `Bool`
- YAML string → `Str`
- YAML null → `nil`
- YAML sequence → `List`
- YAML mapping → `Dict` (keys must be strings; raises otherwise)

```scl
Encoding.fromYaml("count: 42\nname: test\n")
// #{"count": 42, "name": "test"}
```

### Encoding.toToml

Serialize a `Record` or `Dict` value to a TOML string. The top-level value must be a record or dict; raises otherwise.

```scl
Encoding.toToml({ name: "test", count: 42 })
// "count = 42\nname = \"test\"\n"
```

### Encoding.fromToml

Parse a TOML string into a value.

Value mappings:
- TOML integer → `Int`
- TOML float → `Float`
- TOML boolean → `Bool`
- TOML string → `Str`
- TOML datetime → `Str` (ISO 8601 / RFC 3339 format)
- TOML array → `List`
- TOML table → `Dict` (with `Str` keys)

```scl
Encoding.fromToml("name = \"test\"\ncount = 42\n")
// #{"count": 42, "name": "test"}
```

### Encoding.urlEncode

Percent-encode a string for use as a URL component. All characters that are not unreserved URL characters (`A-Z`, `a-z`, `0-9`, `-`, `_`, `.`, `~`) are percent-encoded. Spaces become `%20`.

```scl
Encoding.urlEncode("hello world")  // "hello%20world"
Encoding.urlEncode("a/b?c=d")      // "a%2Fb%3Fc%3Dd"
```

### Encoding.urlDecode

Decode a percent-encoded URL component string. Raises if the input contains malformed percent-encoding or if the decoded bytes are not valid UTF-8.

```scl
Encoding.urlDecode("hello%20world")  // "hello world"
```

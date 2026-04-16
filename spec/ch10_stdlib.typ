// ============================================================================
//  Chapter 10 — The standard library and extern functions
// ============================================================================

#import "preamble.typ": *

= The Standard Library

SCL's standard library is not a language construct; it is a
collection of modules in a package named #raw("Std"), together with
the _extern function_ mechanism by which SCL reaches out to its host
runtime. This chapter describes the interface of the standard
library, the evaluation-time obligations of a host, and the division
of responsibilities between _pure_ and _resource_ externs.

== Extern functions

An _extern function_ is a value whose implementation is supplied by
the host evaluator rather than by an SCL expression. Syntactically, a
binding is declared extern by the #kw("extern") expression:

#align(center)[
    #raw("export let f: (Int) -> Int = extern \"math/square\"")
]

The string argument to #kw("extern") is a _dispatch key_ that names
the host implementation. Extern values are ordinary first-class values
and may be passed around, stored in records, and invoked indirectly.

At evaluation time, an extern invocation consults a fixed partial
function

$ "host" : "Name" times overline("Value") arrow "Value" $

supplied by the deployer. The reference implementation's host
dispatches pure externs synchronously and resource externs via the
run-time plug-in protocol (RTP), as described in Section 10.3.

== Pure versus resource externs

Externs divide into two disjoint classes:

- _Pure externs_ are deterministic functions of their arguments. The
  host is obliged to return the same value on identical inputs; no
  side effects are observable. Examples include #raw("Num.parse"),
  #raw("List.length"), #raw("Encoding.toJson").
- _Resource externs_ are the elimination form of _resources_ — units
  of deployable state. A resource extern returns a pending value
  during initial evaluation; the deployer reconciles the pending
  value against the real world by invoking the corresponding plug-in,
  and re-evaluates the downstream graph with the resolved output.

The type system does not distinguish pure and resource externs: both
have ordinary function types. The distinction is carried entirely by
the host's dispatch behaviour. Resource externs are recognised by
convention through their appearance in the type-level catalogue of
the standard library.

== The run-time plug-in protocol

A resource extern delegates its create, update, and delete operations
to an external process that implements the _run-time plug-in
protocol_ (RTP). The protocol is a small bidirectional RPC over a
Unix socket or TCP connection; the salient operations are:

- _plan_: given the resource's declared inputs and, optionally, the
  previously observed state, return either a concrete output record
  or a _plan_ that describes the change to be applied.
- _apply_: given a plan, perform the real-world change and return the
  resulting output record.
- _delete_: given a previously observed state, destroy the real-world
  resource and return success.

For the purposes of this specification we treat the host as providing
a single mathematical function $"planner"$ mapping (resource-name,
inputs) pairs to outputs or $bot$, leaving the internal mechanics of
the protocol opaque; see the plug-in documentation for operational
details.

== Catalogue of standard modules

The standard library comprises the following modules. Each is
exposed as #raw("Std") $slash$ _ModuleName_ and is imported with
#kw("import") #raw("Std/") _ModuleName_.

=== #raw("Std/Num")

Pure arithmetic over #raw("Int") and #raw("Float"). Representative
signatures:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("parse: (Str) -> Int?")], [Parse a decimal integer; #kw("nil") on parse error.],
        [#raw("parseFloat: (Str) -> Float?")], [Parse a decimal float; #kw("nil") on parse error.],
        [#raw("toStr: (Int) -> Str")], [Canonical decimal.],
        [#raw("min, max: (Int, Int) -> Int")], [Pointwise min and max.],
    )
]

=== #raw("Std/Option")

Elimination forms for optional values that compose cleanly with
`??`:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("unwrap: <T>(T?) -> T")], [Raises if given #kw("nil").],
        [#raw("orElse: <T>(T?, T) -> T")], [The `??` operator as a function.],
        [#raw("map: <A,B>(A?, (A) -> B) -> B?")], [Covariant lift.],
    )
]

=== #raw("Std/List")

Higher-order operations over lists:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("length: <A>([A]) -> Int")], [Cardinality.],
        [#raw("map: <A,B>([A], (A) -> B) -> [B]")], [Pointwise map.],
        [#raw("filter: <A>([A], (A) -> Bool) -> [A]")], [Predicate filter.],
        [#raw("concat: <A>([A], [A]) -> [A]")], [Concatenation.],
        [#raw("fold: <A,B>([A], B, (B, A) -> B) -> B")], [Left fold.],
    )
]

=== #raw("Std/Encoding")

Bidirectional encodings between SCL values and common textual
formats. Representative entries:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("toJson: (Any) -> Str")], [Canonical JSON.],
        [#raw("fromJson: (Str) -> Any")], [Parses to a value; fails for malformed input.],
        [#raw("toYaml, fromYaml")], [Analogous.],
        [#raw("toToml, fromToml")], [Analogous.],
    )
]

The asymmetry of the JSON types — output is any, input is any — is a
consequence of the data interchange: a well-typed SCL program will
typically immediately ascribe the result of #raw("fromJson") to a
record type via #kw("as"), and the static semantics tracks the
optionality of fields structurally from that point onward.

=== #raw("Std/Random")

A _resource_ module producing repeatable pseudo-random values:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("string: ({ seed: Str, length: Int }) -> { value: Str }")], [Random string of fixed length.],
    )
]

Calls to #raw("Std/Random.string") are resources: their outputs are
stable across deploys (as long as the seed does not change) but
pending on initial creation.

=== #raw("Std/Crypto")

Resources for certificate signing requests and signatures:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("csr")], [Create a CSR from a key and subject record.],
        [#raw("signCertificate")], [Produce a certificate signature over a CSR.],
    )
]

=== #raw("Std/DNS")

Resources for DNS record management:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("aRecord")], [Create or update an A-record at a provider.],
        [#raw("cnameRecord")], [Analogous for CNAME.],
    )
]

=== #raw("Std/Container")

Resources for container image and registry operations.

=== #raw("Std/Artifact")

Resources for producing and publishing build artifacts (archives,
binaries).

=== #raw("Std/Package")

Reflection over the importing package: its identity, its manifest
contents, and related metadata. These are pure externs, evaluated at
load time.

=== #raw("Std/Time")

Time-valued resources (capturing the deployment moment) and pure
utilities for formatting and arithmetic.

== Closure under refinement

Every standard function is _refinement-closed_: when its arguments are
of optional type and the propositional engine (Chapter 6) has
narrowed those optionals to their non-optional components, the static
semantics sees a function of non-optional arguments, and the host
sees concrete values. No extern function is required to handle
#kw("nil") unless its signature explicitly declares an optional
parameter.

== Summary

The standard library is a tightly typed façade over a host-provided
environment. The language itself is agnostic to which externs are
registered: a host with no externs is still able to compile and
evaluate every non-#kw("extern") program. The choice of what externs
to provide determines what programs can _do_ at run time, and it is
precisely this choice that distinguishes the SCL compiler from the
SCL deployer.

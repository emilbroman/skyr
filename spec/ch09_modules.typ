// ============================================================================
//  Chapter 9 — Modules, packages, and name resolution
// ============================================================================

#import "preamble.typ": *

= Modules and Packages

An SCL program is not a single expression but a finite collection of
_modules_ arranged in _packages_. This chapter fixes the terminology,
the name-resolution rules, the dependency discipline that governs
inter-module references, and the static checks that apply across
module boundaries.

== Packages, modules, and identities

A _package_ is a collection of modules sharing a common root.
Packages are distinguished by their _package identity_
$"pkg" in "PackageId"$, an opaque token supplied by the loader.
Examples of package identities include the special token #raw("Self")
referring to the package containing the compilation unit, a Git remote
reference such as `github.com/owner/repo@v1.2.3` bound to a
reproducible commit SHA, and the built-in token #raw("Std") naming
the host-provided standard package (§ 9.9).

A _module_ is a text file of SCL source. Its identity has two layers:
its _raw module id_ $r in "RawModuleId"$ is a package-identity paired
with a slash-separated path under the package root, and its _module
id_ $m in "ModuleId"$ is the resolved form in which every intermediate
#kw("Self") prefix has been substituted away. The resolution procedure
is given in Section 9.3. Two modules are _the same module_ iff their
module ids (not their raw ids) are equal.

== Module contents

A module's content is given by the grammar for $S$ in Chapter 3: a
sequence of #kw("import"), #kw("let"), #kw("type"), side-effecting
expression statements, and their #kw("export")-prefixed forms. We
divide a module's statements into five categories:

- _Imports._ Each #kw("import") names another module by a path
  expression interpreted against the importing module's package.
- _Value bindings._ Each non-exported #kw("let") contributes a local
  binding.
- _Type bindings._ Each non-exported #kw("type") contributes a local
  type alias or generic type constructor.
- _Exported bindings._ Each #kw("export") #kw("let") or
  #kw("export") #kw("type") contributes both to the local environment
  of its module and to the _export set_ visible to importers.
- _Side effects._ A bare expression statement is not a binding; it
  evaluates for its effect.

The _export set_ $"exp"(m) = (V, T)$ of a module $m$ is the pair of
its exported value and type bindings, each a finite map from
identifier to type.

== Import resolution

Import statements take the form

$ #kw("import") x_1 slash x_2 slash dots slash x_n $

and resolve as follows. Let the importing module have raw id
$r = ("pkg", [y_1, dots, y_k])$; i.e. its package is $"pkg"$ and its
path under the package root is $y_1 slash dots slash y_k$.

+ If $x_1 = $ #raw("Self"), the import refers to another module in the
  _same package_ as the importer; the import is resolved to the raw
  id $("pkg", [x_2, dots, x_n])$, relative to the package root.
+ Otherwise, $x_1$ is interpreted as an _external package name_;
  the loader resolves it to a $"pkg"' in "PackageId"$, and the import
  is $( "pkg"', [x_2, dots, x_n] )$.

The loader is responsible for turning a raw id into a canonical
module id, recording the relationship in a _module registry_
$"reg" : "RawModuleId" arrow "ModuleId"$. Two raw ids that name the
same canonical module have equal module ids; this is the condition
that prevents duplicate evaluation in the presence of diamond imports.

#remark[
    External packages are rejected unless their identity has been
    registered by the manifest. This is the key rule that isolates the
    compilation of a given project from the unbounded Git namespace
    at large: a freshly cloned repository compiles the same as the
    original iff its manifest pins the same external package
    identities to the same commits.
]

== Scopes and shadowing

A module has a single, flat lexical scope comprising: the names
imported by #kw("import") statements, the names bound by #kw("let")
and #kw("type") statements anywhere in the module, and the built-in
primitive type names (`Int`, `Float`, `Bool`, `Str`, `Path`, `Any`).
Inside a function body, parameter names shadow the enclosing scope.
The body of a #kw("let") $x = e_1 semi e_2$ shadows any enclosing
binding of $x$ within $e_2$ only.

Two module-level bindings to the same name are a _duplicate binding_
error. The primitive type names may be shadowed by user bindings;
such shadowings are legal but are reported to the programmer as
stylistic lints.

An imported module $m'$ brings the exports of $m'$ into the scope of
the importer _qualified_ by the import's last segment. That is,
#kw("import") #raw("Self/Foo/Bar") makes the identifier $#raw("Bar")$
refer to the module — property access $#raw("Bar") . #raw("f")$ then
resolves to $"exp"(m')(#raw("f"))$ if #raw("f") is an exported value,
and similarly for types.

== The dependency graph

Let a module $m$ be given. Its _outgoing dependencies_ are the module
ids of its imports; its _local dependency graph_ is a directed graph
over its bindings where $b_1 arrow b_2$ iff the right-hand side of
$b_2$ mentions the name bound by $b_1$.

Binding resolution proceeds in three phases:

+ _Type binding resolution._ The static system first resolves all
  #kw("type") bindings; these may refer to each other cyclically
  through μ-types (Chapter 4). A strongly connected component (SCC)
  of the type binding dependency graph is resolved together, with
  each SCC introducing a vector of fresh μ-binders.
+ _Value signature inference._ For each #kw("let") whose annotation
  is present, the declared type is used; for each #kw("let") whose
  annotation is absent, the static system enters _signature
  inference_ on its enclosing SCC of the value binding dependency
  graph.
+ _Value body checking._ Once signatures are fixed, each body is
  checked against its signature (or synthesised, if no annotation
  was given).

The bidirectional static system of Chapter 7 is always applied to a
body after its signature is available. This ordering is the source of
SCL's support for mutual recursion without unification-based
inference: within an SCC, each body is type-checked assuming each
sibling's signature, and the overall success of the SCC is verified
post hoc.

== Signature inference for mutual recursion

Within an SCC of size $>= 2$ containing only unannotated bindings,
the static system introduces a fresh type variable $alpha_i$ for the
type of each binding $b_i$ and type-checks the body of $b_i$ in an
environment where the siblings are typed at $alpha_j$. The checking
imposes _free-variable constraints_ of the form $alpha_i = A_i$ once
the body of $b_i$ has been synthesised; provided the constraints are
solvable — i.e. every $alpha_i$ appears on the left of exactly one
equation whose right is $alpha$-free in the remaining variables — the
SCC is accepted and each $alpha_i$ is replaced by $A_i$ in all
sibling bodies.

A cyclic dependency in the type equations is a type error: the
solver does not attempt unification under μ-types for value bindings,
reflecting the design choice that recursive values must be written
as functions (whose return types are explicit by virtue of being
delayed).

== Visibility

The #kw("export") prefix controls which bindings populate a module's
_export set_ $"exp"(m)$ — the map consulted when a foreign module
reconstructs $m$'s public surface, and the map the formal semantics
treats as $m$'s only externally observable content.

Visibility at the level of _name resolution_ is, however, more
permissive than $"exp"(m)$ alone would suggest. The reference
implementation stores every module-level binding — exported or not —
in a common keyspace indexed by module identity, and resolves
qualified accesses $"Alias" . x$ against that common keyspace rather
than against $"exp"(m)$. Consequently, non-exported bindings of an
imported module are _reachable_ through a property access on an
import alias, even though they are not recorded in $"exp"(m)$ and are
not intended by the declaring module to be part of its public API.

Formally, we say that a binding $x$ of module $m$ is _exported_ iff
$x in "exp"(m)$, and _reachable_ iff it is present in $m$'s global
keyspace. Every exported binding is reachable; the converse does not
hold. The type system and the evaluator both consult the reachable
set through property access on an import alias.

#remark[
    This divergence between reachability and export is a known
    laxity of the current reference implementation. A conforming
    alternative implementation may either preserve the laxity or
    tighten property access on import aliases to consult $"exp"(m)$
    exclusively; both choices are valid specifications of SCL under
    the present document. Programs that exploit non-exported bindings
    across module boundaries are advised not to do so: they are not
    guaranteed to continue compiling under future revisions of the
    reference compiler.
]

The analogous laxity applies to _types_. A type declared by a
non-exported #kw("type") statement may appear in the signature of an
exported binding, and such a signature is considered well-formed.
Consumers of the exported binding receive the structural content of
the type (record fields, list/dict constructors, function arities)
regardless of whether its declaring alias is itself exported. The
identifier of a non-exported type alias is preserved verbatim in
displayed types — it survives, as a string, into error messages and
REPL output — but is not a binder that consumers may name in source.

== Cross-module type equality

A type declared in module $m$ by #kw("type") $x chevron.l overline(alpha) chevron.r
colon T$ is identified structurally _after_ substituting the generic
parameters. Two distinct modules may declare textually identical types
that are considered equal under structural equality, subject to the
usual caveats around exception types (which are nominal; see § 4.7)
and around origin identifiers (which are non-propagating across
module boundaries except by explicit sharing; see § 4.2).

== Evaluation order at module level

Modules are evaluated in a topological order induced by the
import graph; a module's statements are then evaluated in the order
induced by the _value_ dependency graph described in § 9.5. A
side-effecting expression statement is scheduled at the point
determined by its read set: it is evaluated after all the bindings it
reads are in scope and before any binding that reads a value it
modifies (by construction there are no such modifications, since the
module environment is immutable; the ordering is nonetheless
well-defined).

Circular imports per se are permitted; only _eager value-level_
dependencies across modules are restricted. Value-level circular
dependencies within a module (from an SCC of size $>= 2$) are
permitted under the signature-inference discipline of § 9.6. A
value-level SCC _across_ modules is not permitted; it would require
signature inference to cross package boundaries, which the reference
implementation does not support. An import cycle whose associated
value-level edges are all lazy (e.g. function bodies referring to
each other across modules) is well-defined.

== The `Std` package

The identifier #raw("Std") is reserved for a built-in package made
available by the host environment. References to #raw("Std") do not
appear in the package manifest; the loader unconditionally resolves
the identifier #raw("Std") to whatever the host has registered under
that name. The contents of #raw("Std") are not fixed by this
specification — the package is a userland convention layered over
the extern mechanism of Chapter 10 — and a host that registers no
standard modules is still conformant. Every other package identifier
must appear in the manifest.

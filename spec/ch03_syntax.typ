// ============================================================================
//  Chapter 3 — Abstract and concrete syntax
// ============================================================================

#import "preamble.typ": *

= Syntax

This chapter fixes both the _abstract_ and the _concrete_ syntax of SCL.
The abstract syntax is the form over which the static and dynamic
semantics are defined; the concrete syntax is what the programmer
writes. The two differ only in a handful of well-confined transformations
applied during parsing, all of which are documented below.

== Abstract syntax

The abstract syntax of SCL comprises three mutually recursive sorts:
_expressions_, _types_, and _module statements_. We write meta-variables
$e$ for expressions, $T$ for type expressions, and $S$ for module
statements. The sort _value_ is introduced in Chapter 8.

=== Expressions

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 2pt),
        align: (right + top, left + top),
        [$e$], [$::=$ $n$ #h(0.6em) _(integer literal)_],
        [],    [$|$ $f$ #h(0.6em) _(float literal)_],
        [],    [$|$ $b$ #h(0.6em) _(boolean literal)_],
        [],    [$|$ $s$ #h(0.6em) _(simple string literal)_],
        [],    [$|$ $chevron.l ... chevron.r$ #h(0.6em) _(interpolated string; see § 3.3)_],
        [],    [$|$ #kw("nil")],
        [],    [$|$ $p$ #h(0.6em) _(path literal)_],
        [],    [$|$ $x$ #h(0.6em) _(variable)_],
        [],    [$|$ $e.x$ #h(0.6em) _(property access)_],
        [],    [$|$ $e ?.$ $x$ #h(0.6em) _(optional property access)_],
        [],    [$|$ $e [ e' ]$ #h(0.6em) _(indexed access)_],
        [],    [$|$ $e (T_1, dots, T_k) (e_1, dots, e_n)$ #h(0.6em) _(call with type arguments)_],
        [],    [$|$ #kw("fn") $chevron.l overline(alpha asgn T) chevron.r ( overline(x colon T) ) . e$ #h(0.6em) _(function abstraction)_],
        [],    [$|$ $e$ #kw("as") $T$ #h(0.6em) _(type ascription / cast)_],
        [],    [$|$ $e_1 plus.o e_2$ #h(0.6em) _(binary operation)_],
        [],    [$|$ $minus.o e$ #h(0.6em) _(unary operation)_],
        [],    [$|$ #kw("if") $(e_1)$ $e_2$ #kw("else") $e_3$],
        [],    [$|$ #kw("if") $(e_1)$ $e_2$],
        [],    [$|$ #kw("let") $x colon T = e_1 semi e_2$],
        [],    [$|$ ${overline(f colon e)}$ #h(0.6em) _(record literal)_],
        [],    [$|$ $[ overline(l) ]$ #h(0.6em) _(list literal; see § 3.5)_],
        [],    [$|$ $hash{overline(e colon e)}$ #h(0.6em) _(dict literal)_],
        [],    [$|$ #kw("exception") $(T)$ | #kw("exception")],
        [],    [$|$ #kw("raise") $e$],
        [],    [$|$ #kw("try") $e$ #h(0.3em) $overline(c)$ #h(0.6em) _(try / catch)_],
        [],    [$|$ #kw("extern") $s$ $colon T$ #h(0.6em) _(extern reference)_],
    )
]

The binary operators $plus.o$ range over
#raw("+"), #raw("-"), #raw("*"), #raw("/"), #raw("=="),
#raw("!="), #raw("<"), #raw("<="), #raw(">"), #raw(">="),
#raw("&&"), #raw("||"), #raw("??"). The unary operators $minus.o$
range over #raw("-") and #raw("!").

A _catch clause_ $c$ is either #kw("catch") $x colon e$ or
#kw("catch") $x (y) colon e$; in the latter form, $y$ binds the
payload of the caught exception. A _path literal_ $p$ is a sequence of
slash-separated path segments as defined in Section 3.4.

=== Types

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 2pt),
        align: (right + top, left + top),
        [$T$], [$::=$ $x$ #h(0.6em) _(type name)_],
        [],    [$|$ $T ?$ #h(0.6em) _(optional)_],
        [],    [$|$ $[ T ]$ #h(0.6em) _(list)_],
        [],    [$|$ $hash {T_1 colon T_2}$ #h(0.6em) _(dict)_],
        [],    [$|$ ${overline(f colon T)}$ #h(0.6em) _(record type)_],
        [],    [$|$ #kw("fn") $chevron.l overline(alpha asgn T) chevron.r ( overline(T) ) $ $T_"ret"$ #h(0.6em) _(function type)_],
        [],    [$|$ $T . x$ #h(0.6em) _(type-level property access)_],
        [],    [$|$ $T chevron.l T_1, dots, T_k chevron.r$ #h(0.6em) _(type application)_],
    )
]

The _type name_ form $x$ names either a _generic type variable_ bound by
an enclosing #kw("fn") or #kw("type") declaration, a _type declaration_
introduced by a #kw("type") statement, or one of the primitive types
`Int`, `Float`, `Bool`, `Str`, `Path`, `Any`, or the bottom type
`Never`. The primitive names are not keywords and share the
identifier namespace with user-declared types; shadowing rules are
those of Chapter 9.

=== Module statements

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 2pt),
        align: (right + top, left + top),
        [$S$], [$::=$ #kw("import") $x_1 slash x_2 slash dots slash x_n$],
        [],    [$|$ #kw("let") $x colon T = e$],
        [],    [$|$ #kw("export") #kw("let") $x colon T = e$],
        [],    [$|$ #kw("type") $x chevron.l overline(alpha) chevron.r colon T$],
        [],    [$|$ #kw("export") #kw("type") $x chevron.l overline(alpha) chevron.r colon T$],
        [],    [$|$ $e$ #h(0.6em) _(side-effecting expression)_],
    )
]

A _module_ is a (possibly empty) sequence of module statements. The
order of statements is syntactic only: modules are evaluated in a
dependency order computed from their bindings (Chapter 9). A side-
effecting expression statement is admitted at module scope because its
_evaluation_, not its _value_, is the point — typically it constructs
resources whose effects are observable in the deployment.

== Concrete syntax and precedence

The concrete syntax is given by a PEG grammar in the reference parser;
the fragment below reproduces its precedence hierarchy. Productions are
listed in ascending precedence order; within each production, binary
operators are left-associative unless otherwise stated.

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        align: (left + horizon, left + horizon),
        [*Level*], [*Operators*],
        [1 — lowest], [#kw("if"), #kw("let"), #kw("fn"), #kw("raise"), #kw("try"), #kw("extern")],
        [2], [#raw("||")],
        [3], [#raw("??")],
        [4], [#raw("&&")],
        [5], [#raw("=="), #raw("!=")],
        [6], [#raw("<"), #raw("<="), #raw(">"), #raw(">=")],
        [7], [#raw("+"), #raw("-")],
        [8], [#raw("*"), #raw("/")],
        [9], [unary #raw("-"), #raw("!")],
        [10 — highest postfix], [property #raw("."), optional #raw("?."), call #raw("(…)"), cast #kw("as"), index #raw("[…]")],
    )
]

The postfix forms at level 10 bind tighter than any infix operator; the
consequence is that in #raw("1 + x as Int") only $x$ is cast, and the
cast binds before the addition — i.e. the expression parses as
$1 + (x$ #kw("as") #raw("Int") $)$. Parentheses around a subexpression override
precedence in the usual way.

The expression-level constructs at level 1 (#kw("if"), #kw("let"),
#kw("fn"), #kw("raise"), #kw("try"), #kw("extern")) each admit an
arbitrary expression as their continuation; they are therefore _right-
associative_ in the sense that the body extends as far rightward as it
can. A #kw("let") body ends at the end of the enclosing expression; a
#kw("fn") body ends likewise. This is why parentheses are not needed
around sequences like #raw("fn(x: Int) x * 2 + 1") — the body is the
whole remainder.

== String interpolation desugaring

An interpolated string literal

#align(center)[#raw("\"s_0{e_1}s_1{e_2}…s_{n-1}{e_n}s_n\"")]

is parsed into an `Interp` AST node whose parts alternate between
string literals $s_i$ and expressions $e_i$. We treat the node as
semantically equivalent to the expression

$ e_1' + e_2' + dots.c + e_(2n+1)' $

where each literal segment is interpreted as the corresponding `Str`
literal and each embedded expression is first evaluated and then coerced
to `Str` (Chapter 8). No explicit `+` operator appears in the abstract
syntax; this equivalence is purely notational.

== Path literals

A path literal is a sequence of slash-separated _path segments_, with
optional leading `.` or `..` segments marking relative paths. Path
segments can be written either as identifiers or as simple string
literals (so that segments may contain arbitrary characters):

#align(center)[
    #table(
        columns: 2,
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [#raw("./foo/bar")], [relative path _foo/bar_ under the module's directory],
        [#raw("../x/y")],    [relative path _x/y_ under the parent directory],
        [#raw("/etc/passwd")], [absolute repo-root path _etc/passwd_],
        [#raw("./\"file with spaces.txt\"")], [quoted segment],
        [#raw(".")],           [the module's directory],
        [#raw("..")],          [the module's parent directory],
        [#raw("/")],           [the repository root],
    )
]

At evaluation time, relative paths are resolved against the directory
containing the module in which the path literal textually appears; the
resulting `Path` value is an absolute repository-root-relative string.

== List comprehensions

List _items_ $l$ extend the expression grammar only inside `[…]`
literals. They are:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 2pt),
        align: (right + top, left + top),
        [$l$], [$::=$ $e$ #h(0.6em) _(element)_],
        [],    [$|$ #kw("if") $(e)$ $l$ #h(0.6em) _(guarded element)_],
        [],    [$|$ #kw("for") $(x$ #kw("in") $e )$ $l$ #h(0.6em) _(iteration)_],
    )
]

A list item may therefore _generate_ more than one or fewer than one
element of the surrounding list. Sections 7 and 8 describe the static
and dynamic semantics of list comprehensions respectively.

== Surface productions elided from the abstract syntax

The parser performs a small set of desugarings that the later chapters
treat implicitly:

- A _field shorthand_ `{ x, y, z }` is elaborated to
  `{ x: x, y: y, z: z }`.
- An #kw("export") prefix is removed and the underlying statement is
  marked as _exported_ in the ASG.
- A doc comment (`/// …`) prefix attached to a field, let-binding or
  type declaration is retained as a doc string on the corresponding
  node.
- An #kw("exception") keyword without payload is treated as though
  written with the payload type `#{Any: Any}?` — i.e. absent — but the
  parser records the distinction, as only an absent payload is
  permitted to omit the binder in a #kw("catch") clause.

Except for field shorthand, none of these elaborations is observable in
well-typed programs.

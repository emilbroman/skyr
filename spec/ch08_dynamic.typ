// ============================================================================
//  Chapter 8 — Dynamic semantics (big-step)
// ============================================================================

#import "preamble.typ": *

= Dynamic Semantics

We now give the _big-step_ operational semantics of SCL. The semantics
is a ternary relation $rho ts e reduces v$ reading _"in the evaluation
environment $rho$, the expression $e$ evaluates to the value $v$"_,
augmented with a separate _raising_ relation $rho ts e raises q$ to
account for exceptions.

The semantics is deterministic: every well-typed expression either
terminates with a unique value, raises a unique exception, or diverges.
Divergence is admitted because SCL has recursive functions and
unbounded list comprehensions; the language provides no syntactic
restriction that would guarantee termination.

== Values

The syntactic category of _values_ is:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 2pt),
        align: (right + top, left + top),
        [$v$], [$::=$ $n$ #h(0.6em) _(integer value)_],
        [],    [$|$ $f$ #h(0.6em) _(float value, NaN excluded)_],
        [],    [$|$ $b$ #h(0.6em) _(boolean value)_],
        [],    [$|$ $s$ #h(0.6em) _(string value)_],
        [],    [$|$ $p$ #h(0.6em) _(path value — absolute, repo-root-relative)_],
        [],    [$|$ #kw("nil")],
        [],    [$|$ $[ overline(v) ]$ #h(0.6em) _(list value)_],
        [],    [$|$ $hash { overline(v colon v) }$ #h(0.6em) _(dict value)_],
        [],    [$|$ ${ overline(f colon v) }$ #h(0.6em) _(record value)_],
        [],    [$|$ $chevron.l $ #kw("fn") $overline(x), e, rho' chevron.r$ #h(0.6em) _(closure)_],
        [],    [$|$ $chevron.l "extern" s chevron.r$ #h(0.6em) _(extern reference)_],
        [],    [$|$ $"exc"(E, v_p)$ #h(0.6em) _(exception value)_],
        [],    [$|$ $bot$ #h(0.6em) _(pending — resource not yet materialised)_],
    )
]

The _pending_ value $bot$ is distinguished: it represents a resource
output that has not yet been computed by the run-time planner. Pending
values are propagated through most operations without triggering
further evaluation; they become concrete values only when the deployer
has reconciled the underlying resource. The dynamic semantics below
takes pending values seriously: most elimination forms have a
dedicated rule for the pending case.

== Environments

An _evaluation environment_ $rho$ is a finite mapping from variable
names to values. We write $rho [x mapsto v]$ for the extension of
$rho$ with $x$ bound to $v$, shadowing any prior binding. The empty
environment is written $diameter$.

Module-level bindings are handled separately through a _module
environment_ $M$ (Chapter 9); at the granularity of expression
evaluation, we treat $M$ as a prefix of $rho$ that has already been
fully computed. The reference implementation evaluates modules in
topological order so that this is always the case.

== Raising exceptions

We write $rho ts e raises q$ for _"$e$ evaluates by raising the
exception value $q$"_. In the rules below, we give the evaluation
rule and suppress the obvious _congruence_ rules that propagate
exceptions through every evaluation position unchanged; formally, for
every rule

$ rho ts e_1 reduces v_1 quad dots quad rho ts e_n reduces v_n
  quad "imply" quad rho ts e reduces v $

there is a dual rule for each $e_i$ saying that if $rho ts e_i raises q$
and $e_1, dots, e_(i-1)$ have already terminated normally, then
$rho ts e raises q$. Only the #kw("try") form intercepts raising;
every other form propagates.

== Literals

The literal forms evaluate to themselves:

#figure(
    grid(
        columns: (1fr, 1fr, 1fr),
        gutter: 1em,
        prooftree(rule(name: [E-Int], $rho ts n reduces n$)),
        prooftree(rule(name: [E-Float], $rho ts f reduces f$)),
        prooftree(rule(name: [E-Bool], $rho ts b reduces b$)),
    ),
    caption: [Numeric and Boolean literals.],
)

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(rule(name: [E-Str], $rho ts s reduces s$)),
        prooftree(rule(name: [E-Nil], $rho ts #kw("nil") reduces #kw("nil")$)),
    ),
    caption: [String and nil literals.],
)

An interpolated string
#raw("\"s_0{e_1}…{e_n}s_n\"") evaluates by first evaluating each
embedded $e_i$, coercing it to string via $"toStr"$, and concatenating
in order:

#figure(
    prooftree(
        rule(
            name: [E-Interp],
            $rho ts #raw("\"s_0{e_1}…{e_n}s_n\"") reduces s_0 + "toStr"(v_1) + dots + "toStr"(v_n) + s_n$,
            $forall i . rho ts e_i reduces v_i$,
        )
    ),
    caption: [Interpolated string.],
)

The coercion $"toStr"$ is the identity on strings, the decimal
representation on #raw("Int") and #raw("Float"), the literal
`true`/`false` on #raw("Bool"), the path string on #raw("Path"), and
the literal `nil` on #kw("nil"); for compound values it is undefined
and a run-time error is raised.

A path literal evaluates by resolving its segments against the
repository root (absolute paths) or the directory of the enclosing
module (relative paths); the result is the normalised
repo-root-relative string.

== Variables

A variable simply looks up its binding in $rho$:

#figure(
    prooftree(
        rule(
            name: [E-Var],
            $rho ts x reduces rho(x)$,
            $x in "dom"(rho)$,
        )
    ),
    caption: [Variable lookup.],
)

By the time evaluation reaches a variable, static scoping has
guaranteed that $x in "dom"(rho)$; the side condition is retained only
to make the rule closed.

== Binary operators

Arithmetic and comparison operators are _strict_ in both operands:

#figure(
    prooftree(
        rule(
            name: [E-Arith],
            $rho ts e_1 plus.o e_2 reduces v_1 plus.o v_2$,
            $rho ts e_1 reduces v_1$,
            $rho ts e_2 reduces v_2$,
            $plus.o in { +, -, *, / \, ==, !=, <, <=, >, >= }$,
        )
    ),
    caption: [Strict binary arithmetic and comparison.],
)

Here, $v_1 plus.o v_2$ denotes the mathematical meaning of the
operator, extended to strings by lexicographic comparison. Division by
zero and integer overflow are _evaluation errors_: the dynamic
semantics is undefined on those inputs. Equality of two compound
values is the structural equality induced on records, lists, and
dicts; heterogeneous comparisons yield $bot$ at the type level and
false at the value level (Section 7.6).

The short-circuit connectives evaluate their right operand only when
the left does not determine the result:

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-And-F],
                $rho ts e_1 #raw("&&") e_2 reduces "false"$,
                $rho ts e_1 reduces "false"$,
            )
        ),
        prooftree(
            rule(
                name: [E-And-T],
                $rho ts e_1 #raw("&&") e_2 reduces v_2$,
                $rho ts e_1 reduces "true"$,
                $rho ts e_2 reduces v_2$,
            )
        ),
    ),
    caption: [Short-circuit conjunction.],
)

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-Or-T],
                $rho ts e_1 #raw("||") e_2 reduces "true"$,
                $rho ts e_1 reduces "true"$,
            )
        ),
        prooftree(
            rule(
                name: [E-Or-F],
                $rho ts e_1 #raw("||") e_2 reduces v_2$,
                $rho ts e_1 reduces "false"$,
                $rho ts e_2 reduces v_2$,
            )
        ),
    ),
    caption: [Short-circuit disjunction.],
)

The _nil-coalescing_ operator `??` short-circuits on the left operand
being non-`nil`:

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-NC-Nil],
                $rho ts e_1 #raw("??") e_2 reduces v_2$,
                $rho ts e_1 reduces #kw("nil")$,
                $rho ts e_2 reduces v_2$,
            )
        ),
        prooftree(
            rule(
                name: [E-NC-Val],
                $rho ts e_1 #raw("??") e_2 reduces v_1$,
                $rho ts e_1 reduces v_1$,
                $v_1 eq.not #kw("nil")$,
            )
        ),
    ),
    caption: [Nil-coalescing.],
)

Unary operators are strict and delegate to the obvious arithmetic or
logical negation; when the operand evaluates to a pending value, the
result is pending.

== Conditionals

The conditional expression evaluates the discriminant first and then
selects a branch:

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-If-T],
                $rho ts #kw("if") (e_c) e_t #kw("else") e_e reduces v$,
                $rho ts e_c reduces "true"$,
                $rho ts e_t reduces v$,
            )
        ),
        prooftree(
            rule(
                name: [E-If-F],
                $rho ts #kw("if") (e_c) e_t #kw("else") e_e reduces v$,
                $rho ts e_c reduces "false"$,
                $rho ts e_e reduces v$,
            )
        ),
    ),
    caption: [Conditional with both branches.],
)

A conditional without an #kw("else") returns #kw("nil") on the false
branch and lifts the true branch into an optional otherwise:

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-If1-T],
                $rho ts #kw("if") (e_c) e_t reduces v$,
                $rho ts e_c reduces "true"$,
                $rho ts e_t reduces v$,
            )
        ),
        prooftree(
            rule(
                name: [E-If1-F],
                $rho ts #kw("if") (e_c) e_t reduces #kw("nil")$,
                $rho ts e_c reduces "false"$,
            )
        ),
    ),
    caption: [Conditional without else.],
)

== Let binding

A #kw("let") binding introduces a fresh variable into the environment
for the evaluation of its body:

#figure(
    prooftree(
        rule(
            name: [E-Let],
            $rho ts #kw("let") x colon T = e_1 semi e_2 reduces v_2$,
            $rho ts e_1 reduces v_1$,
            $rho [x mapsto v_1] ts e_2 reduces v_2$,
        )
    ),
    caption: [Let binding.],
)

== Records, lists, dicts

Record, list, and dict literals evaluate their constituents in source
order and package the results:

#figure(
    prooftree(
        rule(
            name: [E-Record],
            $rho ts { overline(f colon e) } reduces { overline(f colon v) }$,
            $forall i . rho ts e_i reduces v_i$,
        )
    ),
    caption: [Record literal.],
)

#figure(
    prooftree(
        rule(
            name: [E-Dict],
            $rho ts hash { overline(e_k colon e_v) } reduces hash { overline(v_k colon v_v) }$,
            $forall i . rho ts e_(k,i) reduces v_(k,i) #h(0.4em) and #h(0.4em) rho ts e_(v,i) reduces v_(v,i)$,
        )
    ),
    caption: [Dict literal.],
)

A list literal containing _items_ (§ 3.5) has a semantics that depends
on the item form. We define an auxiliary judgment $rho ts l reduces_l
overline(v)$ producing a sequence of values from a single item:

#figure(
    grid(
        columns: 1fr,
        gutter: 1em,
        prooftree(
            rule(
                name: [E-Item-El],
                $rho ts e reduces_l [v]$,
                $rho ts e reduces v$,
            )
        ),
        prooftree(
            rule(
                name: [E-Item-Guard-T],
                $rho ts #kw("if") (e) l reduces_l overline(v)$,
                $rho ts e reduces "true"$,
                $rho ts l reduces_l overline(v)$,
            )
        ),
        prooftree(
            rule(
                name: [E-Item-Guard-F],
                $rho ts #kw("if") (e) l reduces_l [#h(0.1em)]$,
                $rho ts e reduces "false"$,
            )
        ),
        prooftree(
            rule(
                name: [E-Item-For],
                $rho ts #kw("for") (x #kw("in") e) l reduces_l overline(v_1) ++ dots ++ overline(v_n)$,
                $rho ts e reduces [u_1, dots, u_n]$,
                $forall i . rho [x mapsto u_i] ts l reduces_l overline(v_i)$,
            )
        ),
    ),
    caption: [Evaluation of list items (comprehensions).],
)

Then the list literal is the concatenation of the results of all its
items:

#figure(
    prooftree(
        rule(
            name: [E-List],
            $rho ts [ overline(l) ] reduces overline(v_1) ++ dots ++ overline(v_n)$,
            $forall i . rho ts l_i reduces_l overline(v_i)$,
        )
    ),
    caption: [List literal with comprehensions.],
)

== Functions and closures

An anonymous function captures its lexical environment:

#figure(
    prooftree(
        rule(
            name: [E-Fn],
            $rho ts #kw("fn") (overline(x colon T)) . e reduces chevron.l overline(x), e, rho chevron.r$,
        )
    ),
    caption: [Function abstraction.],
)

Generic type binders are erased at run time: the closure records only
the ordinary parameter list and body. This is sound because
well-typedness and type-erasure coexist by Theorem 7.1
(subject reduction) of the reference implementation's companion
metatheory.

A call evaluates the callee, then the arguments in order, and
finally the body in the extended environment:

#figure(
    prooftree(
        rule(
            name: [E-Call],
            $rho ts e_0 (overline(e)) reduces v$,
            $rho ts e_0 reduces chevron.l overline(x), e_b, rho_c chevron.r$,
            $forall i . rho ts e_i reduces v_i$,
            $rho_c [overline(x mapsto v)] ts e_b reduces v$,
        )
    ),
    caption: [Function application.],
)

Calls to _extern_ functions defer to the host environment:

#figure(
    prooftree(
        rule(
            name: [E-Extern],
            $rho ts e_0 (overline(e)) reduces "host"(s, overline(v))$,
            $rho ts e_0 reduces chevron.l "extern" s chevron.r$,
            $forall i . rho ts e_i reduces v_i$,
        )
    ),
    caption: [Extern function call.],
)

The function $"host" : "Name" times overline("Value") arrow "Value"$
is provided by the deployer; the reference evaluator defines a
canonical host that interprets the standard library as described in
Chapter 10 and dispatches all other externs to the plug-in runtime
(RTP). An extern invocation may also introduce a _pending_ value,
which propagates as described in Section 8.12.

== Property and indexed access

Property access reduces on records:

#figure(
    prooftree(
        rule(
            name: [E-Prop],
            $rho ts e . x reduces v_x$,
            $rho ts e reduces { overline(f colon v) }$,
            $x colon v_x in { overline(f colon v) }$,
        )
    ),
    caption: [Property access on a record.],
)

Optional property access returns #kw("nil") on #kw("nil"), and
otherwise behaves as ordinary property access:

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-OProp-Nil],
                $rho ts e ?. x reduces #kw("nil")$,
                $rho ts e reduces #kw("nil")$,
            )
        ),
        prooftree(
            rule(
                name: [E-OProp-Rec],
                $rho ts e ?. x reduces v_x$,
                $rho ts e reduces { overline(f colon v) }$,
                $x colon v_x in { overline(f colon v) }$,
            )
        ),
    ),
    caption: [Optional property access.],
)

Indexed access on a list expects an integer index; out-of-bounds
accesses yield #kw("nil"):

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-Idx-List-In],
                $rho ts e_1 [ e_2 ] reduces v_i$,
                $rho ts e_1 reduces [v_0, dots, v_(n-1)]$,
                $rho ts e_2 reduces i$,
                $0 <= i < n$,
            )
        ),
        prooftree(
            rule(
                name: [E-Idx-List-OOB],
                $rho ts e_1 [ e_2 ] reduces #kw("nil")$,
                $rho ts e_1 reduces [v_0, dots, v_(n-1)]$,
                $rho ts e_2 reduces i$,
                $i < 0 or i >= n$,
            )
        ),
    ),
    caption: [Indexed access on a list.],
)

Indexed access on a dict returns the matched value or #kw("nil"):

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(
            rule(
                name: [E-Idx-Dict-Hit],
                $rho ts e_1 [ e_2 ] reduces v$,
                $rho ts e_1 reduces hash{overline(k colon v)}$,
                $rho ts e_2 reduces k_i$,
                $(k_i colon v) in hash{overline(k colon v)}$,
            )
        ),
        prooftree(
            rule(
                name: [E-Idx-Dict-Miss],
                $rho ts e_1 [ e_2 ] reduces #kw("nil")$,
                $rho ts e_1 reduces hash{overline(k colon v)}$,
                $rho ts e_2 reduces k_*$,
                $k_* in.not "dom"(hash{overline(k colon v)})$,
            )
        ),
    ),
    caption: [Indexed access on a dict.],
)

The reference implementation preserves the invariant that every
well-typed list indexing whose static type is non-optional can only
produce a value (not #kw("nil")). Since the type system always marks
dict and bare list access as producing an optional, the #kw("nil")
outputs above are well-typed exactly at the expected optional type.

== Type cast

A type cast is evaluated as the identity; it has no dynamic
content in the reference evaluator, since SCL is evaluated after
erasure of types:

#figure(
    prooftree(
        rule(
            name: [E-As],
            $rho ts e #kw("as") T reduces v$,
            $rho ts e reduces v$,
        )
    ),
    caption: [Type cast is identity at run time.],
)

== Exceptions

The #kw("exception") keyword produces a _fresh nominal exception
constructor_. Semantically, this is a pair of a globally unique
identifier and an optional payload type:

#figure(
    prooftree(
        rule(
            name: [E-ExcDecl],
            $rho ts #kw("exception") reduces E_(#raw("fresh"))$,
        )
    ),
    caption: [Exception constructor introduction.],
)

The #kw("raise") form evaluates its argument and then transitions to
the _raising_ relation:

#figure(
    prooftree(
        rule(
            name: [E-Raise],
            $rho ts #kw("raise") e raises v$,
            $rho ts e reduces v$,
        )
    ),
    caption: [Raising an exception.],
)

#kw("try")-#kw("catch") is the sole form that intercepts raising. In
the rules below, we write $"matches"(c, q)$ for the predicate that a
catch clause $c$ matches an exception value $q$; this is true iff the
nominal identifier of $q$'s constructor equals that of the clause, or
the clause catches #raw("Any") (i.e. has no constructor constraint).
On a match, $"bind"(c, q, rho)$ denotes the environment extended with
the clause's variables bound to the exception and, if present, its
payload.

#figure(
    grid(
        columns: 1fr,
        gutter: 1em,
        prooftree(
            rule(
                name: [E-Try-Ok],
                $rho ts #kw("try") e #h(0.2em) overline(c) reduces v$,
                $rho ts e reduces v$,
            )
        ),
        prooftree(
            rule(
                name: [E-Try-Catch],
                $rho ts #kw("try") e #h(0.2em) overline(c) reduces v$,
                $rho ts e raises q$,
                $c_i "is the first" c_j in overline(c) "with" "matches"(c_j, q)$,
                $"bind"(c_i, q, rho) ts e_(c_i) reduces v$,
            )
        ),
        prooftree(
            rule(
                name: [E-Try-Rethrow],
                $rho ts #kw("try") e #h(0.2em) overline(c) raises q$,
                $rho ts e raises q$,
                $forall c_j in overline(c) . not "matches"(c_j, q)$,
            )
        ),
    ),
    caption: [Try / catch / re-raise.],
)

The #kw("try") form is the _only_ place where the semantics transitions
back from raising to evaluating; it is also the only place where
side-effecting control flow is observable at the expression level.

== Pending values and resource calls

A call to a _resource function_ (Chapter 10) is distinguished in that
its result may be _pending_ rather than a concrete value. The dynamic
semantics models this with the following rule:

#figure(
    prooftree(
        rule(
            name: [E-Resource-Pending],
            $rho ts e_0 (overline(e)) reduces bot$,
            $rho ts e_0 reduces chevron.l "extern resource" s chevron.r$,
            $forall i . rho ts e_i reduces v_i$,
            $"planner"(s, overline(v)) = bot$,
        )
    ),
    caption: [A resource whose output is not yet known evaluates to pending.],
)

and by a dual rule that produces a concrete value when the planner
reports one. Every ordinary elimination form (property access, indexed
access, string coercion, `??`) has a _pending congruence_ rule:

#figure(
    prooftree(
        rule(
            name: [E-Pending-Prop],
            $rho ts e . x reduces bot$,
            $rho ts e reduces bot$,
        )
    ),
    caption: [Elimination forms propagate pending values.],
)

with the analogous rule for every other elimination form. Equality
with `==` against `nil` is the only elimination form that does _not_
propagate: comparing a pending value to `nil` yields `false` at the
static semantics' discretion, which is what enables the type-system's
nil-comparison refinement (§ 6.5.1) to narrow pending optionals
correctly.

== Module evaluation

A module is evaluated by processing its declarations in a topological
order derived from their dependency graph (Chapter 9). Each
#kw("let") statement evaluates its right-hand side in the partially
constructed module environment $M$ and extends $M$ with the resulting
binding. Exported bindings are recorded in a second environment $M_e$
which is visible to importing modules. #kw("type") statements
contribute nothing to the dynamic environment; they are static-only.

Side-effecting expression statements at module scope evaluate for
their effect; their value is discarded. This is the mechanism by which
a module instantiates resources: the expression evaluates to a
pending-valued record which the deployer observes through the
dependency edges recorded during evaluation.

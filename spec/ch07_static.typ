// ============================================================================
//  Chapter 7 — Static semantics
// ============================================================================

#import "preamble.typ": *

= Static Semantics

The static semantics of SCL is _bidirectional_: it comprises two
judgment forms, _synthesis_ and _checking_, and every expression is
reached by exactly one of the two. In addition, every judgment carries
a _proposition context_ $cal(P)$ and may _emit_ a finite list of
propositions $Pi$ which extend $cal(P)$ in subsequent siblings.

== Judgment forms

Let $Gamma$ denote a _type environment_ mapping variables to types,
$Delta$ denote the _bound map_ of Chapter 5 over type variables, and
$cal(P)$ denote the proven set of Chapter 6. We use the following
judgment forms throughout the chapter:

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 3pt),
        align: (left, left),
        [$Gamma ; cal(P) ts e synth A $ #sym.dagger $Pi$],
        [$e$ _synthesizes_ type $A$ emitting propositions $Pi$.],
        [$Gamma ; cal(P) ts e check A $ #sym.dagger $Pi$],
        [$e$ _checks against_ type $A$ emitting propositions $Pi$.],
    )
]

The dagger notation $#sym.dagger$ separates the type output
from the proposition output; we omit it when no propositions are
emitted. When $cal(P)$ is immaterial we write $Gamma ts e synth A$.

Whenever we write $Gamma ts e synth A$ in a premise, we understand the
proof engine to be available for the derivation: any $"RefinesTo"$
proposition in $cal(P)$ refines the types read out of $Gamma$ and the
synthesised type $A$ before either is used elsewhere.

== Subsumption, synthesis, checking

As is standard, the two modes are linked by the _subsumption_ rule:

#figure(
    prooftree(
        rule(
            name: [T-Sub],
            $Gamma ; cal(P) ts e check A$,
            $Gamma ; cal(P) ts e synth B$,
            $B subtype A$,
        )
    ),
    caption: [Subsumption: synthesis plus subtyping entails checking.],
)

This rule is invoked only when no more specific checking rule applies,
which happens for nearly every expression form whose synthesis and
checking behaviours coincide. The specialised checking rules below are
those for which _expected-type propagation_ yields strictly better
results — typically because the expected type resolves an otherwise
unresolvable ambiguity.

== Literals

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(rule(name: [T-Int], $Gamma ts n synth #raw("Int")$)),
        prooftree(rule(name: [T-Float], $Gamma ts f synth #raw("Float")$)),
    ),
    caption: [Numeric literals.],
)

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(rule(name: [T-Bool], $Gamma ts b synth #raw("Bool")$)),
        prooftree(rule(name: [T-Str], $Gamma ts s synth #raw("Str")$)),
    ),
    caption: [Boolean and string literals.],
)

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1em,
        prooftree(rule(name: [T-Nil], $Gamma ts #kw("nil") synth #raw("Never")?$)),
        prooftree(rule(name: [T-Path], $Gamma ts p synth #raw("Path")$)),
    ),
    caption: [Nil and path literals.],
)

The type #raw("Never")? of #kw("nil") is the least optional type and
is therefore assignable to every optional type; attempting to assign
#kw("nil") to a non-optional target fails S-Opt-Inj in Chapter 5 and
is reported as a type error.

== Variables

#figure(
    prooftree(
        rule(
            name: [T-Var],
            $Gamma ts x synth A$,
            $x colon A in Gamma$,
        )
    ),
    caption: [Variable reference.],
)

If $x$ is unbound in $Gamma$, the expression is ill-formed. Shadowing
rules follow the lexical scoping of Chapter 9; within a single
environment, names are unique.

== Binary operators

The binary operators fall into four families: arithmetic, comparison,
Boolean, and nil-coalescing. Each has its own static rules.

=== Arithmetic

#figure(
    prooftree(
        rule(
            name: [T-Arith-Int],
            $Gamma ts e_1 plus e_2 synth #raw("Int")$,
            $Gamma ts e_1 synth #raw("Int")$,
            $Gamma ts e_2 synth #raw("Int")$,
        )
    ),
    caption: [Integer arithmetic.],
)

#figure(
    prooftree(
        rule(
            name: [T-Arith-Float],
            $Gamma ts e_1 plus e_2 synth #raw("Float")$,
            $Gamma ts e_1 synth A_1$,
            $Gamma ts e_2 synth A_2$,
            $#raw("Float") in {A_1, A_2}$,
            $forall i . #h(0.2em) A_i subtype #raw("Float")$,
        )
    ),
    caption: [Mixed-precision arithmetic lifts to `Float`.],
)

Analogous rules apply for #raw("-"), #raw("*"), #raw("/"). The #raw("+")
operator is additionally overloaded on strings:

#figure(
    prooftree(
        rule(
            name: [T-Str-Concat],
            $Gamma ts e_1 plus e_2 synth #raw("Str")$,
            $Gamma ts e_1 synth #raw("Str")$,
            $Gamma ts e_2 synth #raw("Str")$,
        )
    ),
    caption: [String concatenation with `+`.],
)

=== Comparison

#figure(
    prooftree(
        rule(
            name: [T-Compare],
            $Gamma ts e_1 plus.o e_2 synth #raw("Bool")$,
            $plus.o in { lt, lt.eq, gt, gt.eq }$,
            $Gamma ts e_1 synth A_1$,
            $Gamma ts e_2 synth A_2$,
            $forall i . #h(0.2em) A_i subtype #raw("Float") or A_i subtype #raw("Int")$,
        )
    ),
    caption: [Ordered comparison.],
)

=== Equality and nil-comparison

#figure(
    prooftree(
        rule(
            name: [T-Eq],
            $Gamma ; cal(P) ts e_1 equiv e_2 synth #raw("Bool") #h(0.3em) #sym.dagger #h(0.3em) Pi$,
            $Gamma ; cal(P) ts e_1 synth A_1$,
            $Gamma ; cal(P) ts e_2 synth A_2$,
            $not (A_1 disjoint A_2)$,
            $Pi = "nilCompareProps"(e_1, e_2)$,
        )
    ),
    caption: [Equality, emitting nil-comparison propositions.],
)

The side condition $not (A_1 disjoint A_2)$ rules out trivially-false
comparisons. The propositions $Pi$ are as described in Section 6.5.1.

=== Boolean operators

The `&&` operator types its right operand under the proposition
$"IsTrue"("id"(e_1))$ and wraps any emitted propositions in an
implication from the same antecedent:

#figure(
    prooftree(
        rule(
            name: [T-And],
            $Gamma ; cal(P) ts e_1 #raw("&&") e_2 synth #raw("Bool") #h(0.3em) #sym.dagger #h(0.3em) Pi$,
            $Gamma ; cal(P) ts e_1 synth #raw("Bool") #h(0.2em) #sym.dagger #h(0.2em) Pi_1$,
            $Gamma ; cal(P) union.plus { "IsTrue"(e_1) } ts e_2 synth #raw("Bool") #h(0.2em) #sym.dagger #h(0.2em) Pi_2$,
            $Pi = Pi_1 union { "IsTrue"(e_1) implies p | p in Pi_2 } union "andProps"(e_1, e_2)$,
        )
    ),
    caption: [Conjunction and its propositional side effects.],
)

The rule for `||` is the De Morgan dual: $e_2$ is typed under
$not "IsTrue"(e_1)$.

=== Nil-coalescing

#figure(
    prooftree(
        rule(
            name: [T-NilCoalesce],
            $Gamma ts e_1 ?? e_2 synth B$,
            $Gamma ts e_1 synth A?$,
            $Gamma ts e_2 check A$,
            $B = A$,
        )
    ),
    caption: [The `??` operator collapses optional into non-optional.],
)

The checker's choice of $B = A$ reflects the fact that the left operand
is already known to be optional, and the right must supply a fallback
of the unwrapped type.

== Conditional expressions

#figure(
    prooftree(
        rule(
            name: [T-If],
            $Gamma ; cal(P) ts #kw("if") (e_c) e_t #kw("else") e_e synth A #h(0.2em) #sym.dagger #h(0.2em) Pi$,
            $Gamma ; cal(P) ts e_c synth #raw("Bool") #h(0.2em) #sym.dagger #h(0.2em) Pi_c$,
            $Gamma ; cal(P) union.plus Pi_c union { "IsTrue"(e_c) } ts e_t synth A #h(0.2em) #sym.dagger #h(0.2em) Pi_t$,
            $Gamma ; cal(P) union.plus Pi_c union { not "IsTrue"(e_c) } ts e_e check A #h(0.2em) #sym.dagger #h(0.2em) Pi_e$,
            $Pi = "wrap"_(cal(P), e_c) (Pi_c, Pi_t, Pi_e)$,
        )
    ),
    caption: [Conditional, with condition-indexed proposition propagation.],
)

The wrapper function $"wrap"$ takes the condition, the propositions
emitted by each branch, and returns the set of implications exported by
the conditional as a whole. In the absence of an #kw("else") clause,
the result type is wrapped in one layer of optional:

#figure(
    prooftree(
        rule(
            name: [T-If-NoElse],
            $Gamma ; cal(P) ts #kw("if") (e_c) e_t synth A? #h(0.2em) #sym.dagger #h(0.2em) Pi$,
            $Gamma ; cal(P) ts e_c synth #raw("Bool") #h(0.2em) #sym.dagger #h(0.2em) Pi_c$,
            $Gamma ; cal(P) union.plus Pi_c union { "IsTrue"(e_c) } ts e_t synth A #h(0.2em) #sym.dagger #h(0.2em) Pi_t$,
        )
    ),
    caption: [Conditional without `else` branch.],
)

However, when the expected type is itself optional, the checker avoids
double-wrapping: the rule T-If-NoElse-Check below elides the extra
optional layer when the expected type is already $A?$.

#figure(
    prooftree(
        rule(
            name: [T-If-NoElse-Check],
            $Gamma ; cal(P) ts #kw("if") (e_c) e_t check A? #h(0.2em) #sym.dagger #h(0.2em) Pi$,
            $Gamma ; cal(P) ts e_c synth #raw("Bool") #h(0.2em) #sym.dagger #h(0.2em) Pi_c$,
            $Gamma ; cal(P) union.plus Pi_c union { "IsTrue"(e_c) } ts e_t check A #h(0.2em) #sym.dagger #h(0.2em) Pi_t$,
        )
    ),
    caption: [Checking mode avoids double-wrapping `Optional`.],
)

== Let-bindings

#figure(
    prooftree(
        rule(
            name: [T-Let],
            $Gamma ; cal(P) ts #kw("let") x colon A = e_1 semi e_2 synth B$,
            $Gamma ; cal(P) ts e_1 check A #h(0.2em) #sym.dagger #h(0.2em) Pi_1$,
            $Gamma, x colon A ; cal(P) union.plus Pi_1 ts e_2 synth B$,
        )
    ),
    caption: [Typed `let` with explicit annotation.],
)

When the annotation is omitted, $A$ is replaced by the synthesised type
of $e_1$.

== Records, lists, dicts

Records synthesise the type of each field independently:

#figure(
    prooftree(
        rule(
            name: [T-Record],
            $Gamma ts {overline(f colon e)} synth {overline(f colon A)}$,
            $forall i . #h(0.2em) Gamma ts e_i synth A_i$,
        )
    ),
    caption: [Record literal.],
)

With an expected record type the checker switches to expected-type
propagation, pushing the expected field type into the corresponding
field expression.

List literals synthesise the type of their first element and check
subsequent elements against it:

#figure(
    prooftree(
        rule(
            name: [T-List],
            $Gamma ts [e_1, dots, e_n] synth [A]$,
            $Gamma ts e_1 synth A$,
            $forall i > 1 . #h(0.2em) Gamma ts e_i check A$,
        )
    ),
    caption: [Non-empty list literal.],
)

The empty list synthesises $[ #raw("Never") ]$, which is assignable to
$[B]$ for any $B$.

List items in comprehension position check as follows: a #kw("for")
item over an iterable of type $[C]$ binds its iteration variable at $C$
and checks its body at the surrounding element type; an #kw("if") item
checks its condition at #raw("Bool") and its body at the surrounding
element type.

Dict literals are analogous; the first entry's key and value types
govern subsequent entries' checking.

== Functions and calls

An anonymous function with explicit parameter types synthesises a
monomorphic function type:

#figure(
    prooftree(
        rule(
            name: [T-Fn-Mono],
            $Gamma ts #kw("fn") (x_1 colon A_1, dots, x_n colon A_n) . e synth (overline(A)) arrow B$,
            $Gamma, overline(x colon A) ts e synth B$,
        )
    ),
    caption: [Monomorphic function.],
)

In checking mode parameter annotations are optional. Each provided
annotation must be a supertype of the corresponding expected
parameter type (contravariance of $arrow$ in its domain), each
omitted annotation is filled in by the expected type, and the body
is checked against the expected return type. Let $A_i$ denote the
effective parameter type — the provided annotation when present, or
$A'_i$ when omitted:

#figure(
    prooftree(
        rule(
            name: [T-Fn-Check],
            $Gamma ts #kw("fn") (x_1 colon A_1^?, dots, x_n colon A_n^?) . e check (overline(A')) arrow B$,
            $forall i . #h(0.2em) A'_i subtype A_i$,
            $Gamma, overline(x colon A) ts e check B$,
        )
    ),
    caption: [Checking mode propagates expected parameter and return types.],
)

A generic function introduces fresh type variables and checks its body
under the extended bound map:

#figure(
    prooftree(
        rule(
            name: [T-Fn-Poly],
            $Gamma ts #kw("fn") chevron.l overline(alpha asgn A') chevron.r (overline(x colon A)) . e synth forall overline(alpha asgn A') . (overline(A)) arrow B$,
            $Delta' = Delta, overline(alpha asgn A')$,
            $Gamma, overline(x colon A) ; Delta' ts e synth B$,
        )
    ),
    caption: [Polymorphic function.],
)

A _call site_ synthesises the return type of its callee after
instantiation. We give the rule in its polymorphic form; the
monomorphic form is the special case with empty type arguments.

#figure(
    prooftree(
        rule(
            name: [T-Call],
            $Gamma ts f chevron.l overline(T) chevron.r (overline(e)) synth B[overline(T) slash overline(alpha)]$,
            $Gamma ts f synth forall overline(alpha asgn A') . (overline(A)) arrow B$,
            $|overline(T)| = |overline(alpha)| or "inferable"$,
            $forall i . #h(0.2em) T_i subtype A'_i$,
            $forall j . #h(0.2em) Gamma ts e_j check A_j [overline(T) slash overline(alpha)]$,
        )
    ),
    caption: [Call, with type-argument inference or annotation.],
)

The predicate _inferable_ holds when $overline(T)$ can be computed from
the argument types by the bound-collection algorithm of Chapter 5.7.

== Access forms

=== Property access

#figure(
    prooftree(
        rule(
            name: [T-Prop],
            $Gamma ts e.x synth A$,
            $Gamma ts e synth {dots, x colon A, dots}$,
        )
    ),
    caption: [Property access on a record type.],
)

=== Optional chaining

#figure(
    prooftree(
        rule(
            name: [T-OptProp],
            $Gamma ; cal(P) ts e ?. x synth B? #h(0.2em) #sym.dagger #h(0.2em) Pi$,
            $Gamma ; cal(P) ts e synth {dots, x colon B, dots} ? #h(0.2em) #sym.dagger #h(0.2em) Pi_e$,
            $Pi = Pi_e union "optChainProps"(e, x, B)$,
        )
    ),
    caption: [Optional chaining, emitting refinement propositions.],
)

The helper $"optChainProps"$ produces the implications described in
Section 6.5.4.

=== Indexed access

#figure(
    prooftree(
        rule(
            name: [T-Idx-List],
            $Gamma ts e [ e' ] synth A?$,
            $Gamma ts e synth [A]$,
            $Gamma ts e' synth #raw("Int")$,
        )
    ),
    caption: [Indexed access on a list.],
)

#figure(
    prooftree(
        rule(
            name: [T-Idx-Dict],
            $Gamma ts e [ e' ] synth A_v ?$,
            $Gamma ts e synth hash {A_k colon A_v}$,
            $Gamma ts e' check A_k$,
        )
    ),
    caption: [Indexed access on a dict.],
)

In both cases the result is optional, reflecting that the index may not
correspond to a present element.

== Type cast

#figure(
    prooftree(
        rule(
            name: [T-As],
            $Gamma ts e #kw("as") A synth A$,
            $Gamma ts e check A$,
        )
    ),
    caption: [Type cast: checked re-ascription to the target type.],
)

The form $e #kw("as") A$ is a compile-time re-ascription: $e$ is
checked against the target type $A$, and the cast synthesizes $A$.
Its purpose is to steer inference toward a specific type when the
context would otherwise leave it underdetermined, and to tighten the
static type of a value that is already compatible with $A$ (for
example, ascribing a concrete type onto a broader join). The cast is
_not_ a runtime coercion: evaluation preserves the value unchanged,
and no dynamic check is performed. Consequently, $#kw("as")$ cannot
narrow a value of type #raw("Any") to a sharper type — the check
$Gamma ts e check A$ must already succeed statically.

== Extern references

An extern reference introduces a host-supplied value at a declared
type:

#figure(
    prooftree(
        rule(
            name: [T-Extern],
            $Gamma ts #kw("extern") s colon T synth A$,
            $A = "interp"(T)$,
        )
    ),
    caption: [Extern reference.],
)

where $"interp"(T)$ denotes the interpretation of the surface type
expression $T$ as an internal type, in the sense of Chapter 4. No
typing obligation is imposed on the dispatch key $s$; the static
system treats the declared type as authoritative and relies on the
host to provide a value of that type at run time. Mis-declaration of
an extern's type is undetectable statically and undefined at run
time: the host is not required to validate its return values against
the declared type, and the dynamic semantics places no obligation on
implementations to report a type mismatch as an evaluation error.
Such a program's behaviour is unspecified.

== Exceptions

An #kw("exception") expression generates a fresh exception identifier
and synthesises a function type:

#figure(
    prooftree(
        rule(
            name: [T-Exc],
            $Gamma ts #kw("exception") (A) synth (A) arrow E$,
        )
    ),
    caption: [Exception constructor with payload.],
)

#figure(
    prooftree(
        rule(
            name: [T-Exc-Nil],
            $Gamma ts #kw("exception") synth E$,
        )
    ),
    caption: [Exception value without payload.],
)

The exception identifier $E$ is chosen fresh. A #kw("raise") expression
is polymorphic in its result type, reflecting that raising transfers
control away from the expression's context:

#figure(
    prooftree(
        rule(
            name: [T-Raise],
            $Gamma ts #kw("raise") e synth #raw("Never")$,
            $Gamma ts e synth E$,
        )
    ),
    caption: [Raise produces `Never`.],
)

A #kw("try") expression checks its body and each of its catch clauses
at the same expected type $A$:

#figure(
    prooftree(
        rule(
            name: [T-Try],
            $Gamma ts #kw("try") e overline(c) check A$,
            $Gamma ts e check A$,
            $forall c_i . #h(0.2em) Gamma ts c_i check A$,
        )
    ),
    caption: [Try/catch.],
)

Each catch clause binds its handler scope with the exception's payload
type (if any):

#figure(
    prooftree(
        rule(
            name: [T-Catch-Payload],
            $Gamma ts #kw("catch") x (y) colon e check A$,
            $x colon (B) arrow E in Gamma$,
            $Gamma, y colon B ts e check A$,
        )
    ),
    caption: [Catch clause binding the payload.],
)

#figure(
    prooftree(
        rule(
            name: [T-Catch-NoPayload],
            $Gamma ts #kw("catch") x colon e check A$,
            $x colon E in Gamma$,
            $Gamma ts e check A$,
        )
    ),
    caption: [Catch clause without payload binder.],
)

== Recursive and mutually recursive bindings

At module scope, dependencies between #kw("let") bindings form a
directed graph; the compiler computes strongly connected components
(Chapter 9) and processes each component in reverse topological order.
Within an SCC that contains a single non-function binding, the cycle
is rejected (no runtime recursion exists for non-function values).
Within an SCC of function bindings, each binding is given a fresh
type variable and each body is checked under an environment in which
those type variables appear with their respective names. Constraints
on the variables are collected from the bodies and solved at the end.
The resulting types are wrapped in μ-binders where self-reference is
observed.

This procedure implements the standard algorithm of mutual recursion
resolution for structural type systems and is described in the
reference implementation's `checker.rs` under the names `FreeVarConstraints`
and `tighten_lower`/`tighten_upper`.

// ============================================================================
//  Chapter 5 — Subtyping and assignability
// ============================================================================

#import "preamble.typ": *

= Subtyping and Assignability

We formalise the relation $B subtype A$ read _"$B$ is assignable to
$A$"_. The relation is reflexive and transitive, depth- and
width-subtyping for records, covariant on list and optional
constructors, and covariant–contravariant on function types. It is
decidable by a straightforward structural algorithm, which is precisely
what the reference implementation runs.

== The assignability judgment

We write
$ Delta ts B subtype A $
for the judgment _"$B$ is assignable to $A$ under the bound map
$Delta$"_. The bound map $Delta$ records upper bounds on type
variables encountered during the traversal; at the top level it is
usually empty. We often elide $Delta$ when it is empty, writing
$B subtype A$ for $diameter ts B subtype A$.

== Structural axioms

We start with the generic structural rules.

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1.2em,
        prooftree(
            rule(
                name: [S-Refl],
                $Delta ts A subtype A$,
            )
        ),
        prooftree(
            rule(
                name: [S-Top],
                $Delta ts B subtype #raw("Any")$,
            )
        ),
    ),
    caption: [Reflexivity and the top rule.],
)

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1.2em,
        prooftree(
            rule(
                name: [S-Bot],
                $Delta ts #raw("Never") subtype A$,
            )
        ),
        prooftree(
            rule(
                name: [S-Unfold],
                $Delta ts B subtype A$,
                $Delta ts "unfold"(B) subtype "unfold"(A)$,
            )
        ),
    ),
    caption: [Bottom and iso-recursive unfolding.],
)

The _unfold_ operation is defined to apply to a $mu$-type exactly once,
and to be the identity on every other type former. The rule S-Unfold
is therefore algorithmically immediate: both sides are unfolded up to
one level before the remaining rules are tried.

== Optional types

Optional types are transparent to subtyping in both directions: an
optional on the right is stripped if the left is optional, and a
non-optional on the right is injected into an optional on the left.

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1.2em,
        prooftree(
            rule(
                name: [S-Opt-Opt],
                $Delta ts B? subtype A?$,
                $Delta ts B subtype A$,
            )
        ),
        prooftree(
            rule(
                name: [S-Opt-Inj],
                $Delta ts B subtype A?$,
                $Delta ts B subtype A$,
            )
        ),
    ),
    caption: [Optional subtyping.],
)

Note the asymmetry: there is no converse rule allowing an optional to
be assigned to a non-optional target, reflecting the fundamental
requirement that the target may not accept `nil`.

== Lists and dicts

List and dict constructors are covariant in every position:

#figure(
    grid(
        columns: (1fr, 1fr),
        gutter: 1.2em,
        prooftree(
            rule(
                name: [S-List],
                $Delta ts [B] subtype [A]$,
                $Delta ts B subtype A$,
            )
        ),
        prooftree(
            rule(
                name: [S-Dict],
                $Delta ts hash {B_k colon B_v} subtype hash {A_k colon A_v}$,
                $Delta ts B_k subtype A_k$,
                $Delta ts B_v subtype A_v$,
            )
        ),
    ),
    caption: [List and dict variance.],
)

Since dict values are heterogeneous only within the covariance of their
declared value type, covariance on the key position is unproblematic:
the sole elimination form for dicts (indexed access $d [k]$) consumes
a key of the dict's key type and returns an optional of the dict's
value type, which is the classical reader's variance.

== Records — width and depth

Records carry both width- and depth-subtyping:

#figure(
    prooftree(
        rule(
            name: [S-Record],
            $Delta ts {overline(f colon B)} subtype {overline(g colon A)}$,
            $forall g_i in "dom"(A) . g_i in "dom"(B)$,
            $forall g_i in "dom"(A) . Delta ts B(g_i) subtype A(g_i)$,
        )
    ),
    caption: [Width and depth subtyping on records.],
)

The first premise is width subtyping — every field _required_ by the
target must be present in the source; the second premise is depth
subtyping — shared fields must be individually assignable.

A noteworthy refinement of the width rule is that a target field of
_optional_ type is not required in the source. Formally:

#figure(
    prooftree(
        rule(
            name: [S-Record-Opt-Drop],
            $Delta ts {overline(f colon B)} subtype {overline(g colon A)}$,
            $forall g_i in "dom"(A) .\ (g_i in "dom"(B) and Delta ts B(g_i) subtype A(g_i)) \ or A(g_i) = A'_i ?$,
        )
    ),
    caption: [Missing fields are tolerated when the target type is optional.],
)

This rule makes optional fields genuinely optional at the boundary.

== Function types

Function types follow the standard covariant–contravariant discipline on
monomorphic types:

#figure(
    prooftree(
        rule(
            name: [S-Fn-Mono],
            $Delta ts (overline(B)) arrow B_"ret" subtype (overline(A)) arrow A_"ret"$,
            $|overline(A)| = |overline(B)|$,
            $forall i . #h(0.2em) Delta ts A_i subtype B_i$,
            $Delta ts B_"ret" subtype A_"ret"$,
        )
    ),
    caption: [Function subtyping, monomorphic case.],
)

On polymorphic types, subtyping follows the _F_-sub discipline: both
sides must quantify over the same number of type variables; the
_bounds_ are contravariant; and the bodies are compared under an
α-renaming.

#figure(
    prooftree(
        rule(
            name: [S-Fn-Poly],
            $Delta ts forall overline(beta asgn B') . (overline(B)) arrow B_"ret" subtype forall overline(alpha asgn A') . (overline(A)) arrow A_"ret"$,
            $|overline(alpha)| = |overline(beta)|$,
            $forall i . #h(0.2em) Delta ts A'_i subtype B'_i$,
            $Delta, overline(alpha asgn A') ts (overline(B)) arrow B_"ret" [overline(alpha) slash overline(beta)] subtype (overline(A)) arrow A_"ret"$,
        )
    ),
    caption: [Function subtyping, polymorphic case (F-sub rule).],
)

The polymorphic discipline admits the usual rules of instantiation and
generalisation; two specialised rules apply when the source has more
(or fewer) type parameters than the target:

- A generic source may be instantiated to match a monomorphic target:
  the type arguments are inferred by the _bound-collection_ algorithm
  of Section 5.7.
- A generic target with an empty source instantiates the target at its
  declared bounds and recurses on the resulting monomorphic
  comparison. (In other words, supplying a polymorphic value to a
  monomorphic slot first chooses the most-general instantiation.)

== Bound collection and generic instantiation

At a call site to a generic function

$ f : forall overline(alpha asgn A'). (overline(A)) arrow A_"ret" $

with arguments of inferred types $overline(B)$, the system computes
_lower_ and _upper_ bounds on each $alpha$ by walking the parameter
types contravariantly:

$
"collect"(B_i, A_i, "contra") quad "for each" i
$

Every occurrence of $alpha in "ftv"(A_i)$ in a contravariant
position contributes $B_i$ as a lower bound; every occurrence in a
covariant position contributes $B_i$ as an upper bound. After all
arguments are processed, the join of the collected lower bounds is
taken as the _solution_ for $alpha$; it is then checked against the
declared upper bound $A'$, and the solved type arguments are
substituted into the function's signature.

The algorithm is the classical _local type inference_ of Pierce &
Turner, specialised to the depth that SCL requires. Crucially it
_never_ guesses: the lower-bound join fails immediately when the
collected constraints are incompatible, and the failure is reported as
a type error.

== Disjointness

Two types $A$ and $B$ are _disjoint_, written $A disjoint B$, when
neither $A subtype B$ nor $B subtype A$ holds. Disjointness is used by
the static semantics of equality (Chapter 7) to warn about comparisons
whose outcome is trivially false.

== Algorithmic summary

The subtyping algorithm implemented by the reference compiler is the
syntactic closure of the above rules under the following cases, each
performed in the order listed:

+ If either side is a μ-type, unfold it.
+ If $A = $ #raw("Any") or $B = $ #raw("Never"), succeed.
+ If the shapes are equal, recurse into their component types with the
  appropriate variances.
+ If $A = A' ?$ and $B = B' ?$, recurse on $B' subtype A'$.
+ If $A = A' ?$ and $B$ is not optional, recurse on $B subtype A'$.
+ If $B$ is a type variable with an upper bound $U$ in $Delta$, recurse
  on $U subtype A$.
+ Otherwise, fail.

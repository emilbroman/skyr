// ============================================================================
//  Appendix — Summary of judgments
// ============================================================================

#import "preamble.typ": *

#set heading(numbering: "A.1")
#counter(heading).update(0)

= Summary of Judgments

This appendix gathers, for reference, the principal judgment forms of
the preceding chapters in one place.

== Subtyping

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 4pt),
        align: (left + top, left + top),
        [$Delta ts B subtype A$], [_"$B$ is assignable to $A$ under bound map $Delta$."_ Chapter 5.],
        [$A disjoint B$], [_"$A$ and $B$ are disjoint."_ Chapter 5.],
    )
]

== Propositions

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 4pt),
        align: (left + top, left + top),
        [$cal(P) ts p$], [_"$p$ is derivable from the proven set $cal(P)$ by forward chaining."_ Chapter 6.],
        [$cal(P) union.plus Pi$], [_"The proven set obtained by extending $cal(P)$ with the proposition set $Pi$."_ Chapter 6.],
        [$"refine"_cal(R)(A)$], [_"The type $A$ with the refinement map $cal(R)$ applied structurally."_ Chapter 6.],
    )
]

== Static semantics

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 4pt),
        align: (left + top, left + top),
        [$Gamma ; cal(P) ts e synth A #sym.dagger Pi$], [_"$e$ synthesizes type $A$ and emits proposition set $Pi$."_ Chapter 7.],
        [$Gamma ; cal(P) ts e check A #sym.dagger Pi$], [_"$e$ checks against $A$ and emits $Pi$."_ Chapter 7.],
    )
]

== Dynamic semantics

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: none,
        inset: (x: 6pt, y: 4pt),
        align: (left + top, left + top),
        [$rho ts e reduces v$], [_"$e$ evaluates to $v$ under environment $rho$."_ Chapter 8.],
        [$rho ts e raises q$], [_"$e$ raises exception $q$ under environment $rho$."_ Chapter 8.],
        [$rho ts l reduces_l overline(v)$], [_"List item $l$ produces the sequence $overline(v)$."_ Chapter 8.],
    )
]

== Metatheoretic propositions

The specification states without proof the following metatheoretic
propositions. Each is stated in the body of the corresponding chapter
and is intended as an invitation to future machine-checked work; none
are load-bearing on the reference implementation.

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 5pt),
        align: (left + top, left + top),
        [*Proposition*], [*Statement*],
        [Propositional soundness], [A typing derivation in the refinement-aware system is sound against the structural fragment: every well-typed program in the former is well-typed in the latter, with possibly less sharp types.],
        [Subject reduction], [If $Gamma ts e synth A$ and $diameter ts e reduces v$, then $diameter ts v check A$; i.e. evaluation preserves types.],
        [Progress], [If $diameter ts e synth A$ and $e$ is not a value, then either $diameter ts e reduces v$ for some $v$, or $diameter ts e raises q$ for some $q$, or evaluation of $e$ diverges.],
    )
]

== Notation index

For convenience, the principal notational conventions are repeated
here.

#align(center)[
    #table(
        columns: (auto, 1fr),
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        align: (left + top, left + top),
        [*Notation*], [*Meaning*],
        [$A, B$], [Types.],
        [$alpha, beta$], [Type variables.],
        [$mu alpha . A$], [Iso-recursive type.],
        [$A?$], [Optional type.],
        [$[A]$], [List type.],
        [$hash{A_k colon A_v}$], [Dict type.],
        [${overline(f colon A)}$], [Record type.],
        [$forall overline(alpha asgn A) . (overline(A)) arrow B$], [Polymorphic function type.],
        [$id$], [Origin identifier.],
        [$"IsTrue"(id)$, $"RefinesTo"(id, A)$], [Atomic propositions.],
        [$Gamma$], [Type environment.],
        [$Delta$], [Bound map over type variables.],
        [$cal(P)$], [Proven set.],
        [$cal(R)$], [Refinement map derived from $cal(P)$.],
        [$rho$], [Evaluation environment.],
        [$v$], [Value.],
        [$q$], [Exception value.],
        [$bot$], [Pending value.],
        [$top$ / #raw("Any")], [Top type.],
        [#raw("Never")], [Bottom type.],
    )
]

// ============================================================================
//  Chapter 1 — Introduction
// ============================================================================

#import "preamble.typ": *

= Introduction

SCL is the configuration language of the _Skyr_ infrastructure platform.
Programs in SCL are read by the platform to produce a _deployment_: a set
of resources, their dependencies, and the effects required to bring the
real world into correspondence with the program. The language is
therefore specialised for declarative description rather than for
arbitrary computation: it has no mutable state, no arbitrary side effects
and no unbounded control constructs. What it does have is a rich type
system designed to make compositional reuse of configuration fragments
safe and ergonomic.

The remainder of this chapter fixes conventions and sketches the shape
of the formalism.

== Design principles

Three design principles drive the SCL language.

_Structural, not nominal, typing._ SCL has no declaration-site
nominality: two record types with the same fields are the same type,
regardless of whether they were introduced via `type` declarations or
constructed in situ. This is crucial for a language whose main job is
fitting together independently authored configuration modules.

_Bidirectional inference._ SCL relies on the ambient context whenever
one is available. A function parameter without a type annotation is
legal precisely when the type can be read off the expected shape at the
call site. Programmers therefore write annotations where they are
genuinely load-bearing and omit them elsewhere.

_Refinement through control flow._ Optional values are pervasive in
configuration (a port may be present or absent; a DNS override may be
configured or not). A purely structural system would force these to be
awkwardly unpacked; SCL instead tracks _propositions_ about the origin
of values through Boolean tests, narrowing types on the branches where
the tests succeed. A variable of type `Int?` checked to be non-`nil` is
regarded as `Int` in the `then` branch.

== Conventions

Throughout this document we use:
- #env, #eenv, #h(0.2em) and $Delta$ for environments mapping names to
  types or values, following the standard typing literature;
- $e$, $e'$, $e_1$, #h(0.1em) etc. for expressions;
- $A$, $B$, $T$, $S$ for types; $x$, $y$ for variables; $v$ for values;
- $p$, $q$ for propositions;
- $tau$, $sigma$ for type schemes and $alpha$, $beta$ for type variables
  introduced by generic abstraction;
- $chevron.l alpha^id chevron.r$ to denote a type variable with origin
  identifier $id$, when the identifier matters for propositional
  refinement; elsewhere we elide it.

The statement $Gamma ts e synth A$ reads _"from_ #env _the expression_ $e$
_synthesizes type_ $A$_"_, and $Gamma ts e check A$ reads _"from_ #env
_the expression_ $e$ _checks against type_ $A$_"_. We reserve the
horizontal bar for inference rules, writing

#align(center)[
    #prooftree(
        rule(
            name: [Name-of-Rule],
            $Gamma ts e synth A$,
            $Gamma ts e_1 synth B$,
            $#env, x colon B ts e_2 synth A$,
        )
    )
]

with the rule's conclusion below the bar and its premises above.

Big-step evaluation judgments are written $eenv ts e reduces v$,
and exceptional termination is written $eenv ts e raises.double epsilon$
where $epsilon$ is an exception value. Evaluation rules use the same
layout as typing rules.

== Relationship to the reference implementation

A reference implementation of SCL is maintained in the `sclc` crate of
the Skyr repository. The formal system in this document is intended to
agree with the reference implementation on all well-typed programs.
Where the implementation makes choices that are either
implementation-specific (_e.g._ the choice of `BTreeMap` for record
field iteration order) or governed by a diagnostic policy
(_e.g._ whether a given error is reported once or many times), the
formal system is silent.

The reference implementation is also the authority on _diagnostic_
quality: the spec does not define error messages, only which programs
are ill-formed.

== Roadmap

After this introduction, the specification is organised top-down from
concrete syntax to denotations:

+ #strong[Chapter 2] fixes the lexical structure: tokens, whitespace,
  comments, string and path literals, and the interaction between
  string interpolation and the brace counter.
+ #strong[Chapter 3] defines both the abstract and concrete syntax of
  SCL, including a precedence table for the expression grammar and the
  module-level statements.
+ #strong[Chapter 4] introduces the type language, including μ-types
  for recursion and the type-variable machinery needed for generics.
+ #strong[Chapter 5] develops subtyping: width- and depth-subtyping for
  records, variance for functions, and the treatment of `Any` and
  `Never`.
+ #strong[Chapter 6] develops the propositional refinement system: the
  language of propositions, the forward-chaining proof engine, and the
  refinement map.
+ #strong[Chapter 7] combines chapters 4–6 into the bidirectional type
  system itself, giving synthesis and checking rules for every
  expression form.
+ #strong[Chapter 8] presents the big-step evaluation semantics,
  including closure capture, exception propagation, and the treatment
  of resource effects.
+ #strong[Chapter 9] describes programs in the large: modules,
  packages, imports, and the strongly-connected-component-based
  resolution of mutually recursive globals.
+ #strong[Chapter 10] gives a brief survey of the standard library and
  its embedding as _external functions_ in the dynamic semantics.
+ Appendix A collects every judgment form defined in the body of the
  document in a single reference.

// ============================================================================
//  A Formal Specification of the Skyr Configuration Language (SCL)
//  ---------------------------------------------------------------------------
//  This document is the authoritative definition of SCL: its lexical
//  structure, abstract and concrete syntax, static and dynamic semantics,
//  and its system of propositional type refinement. It is intended both as
//  reference material for implementers of alternative front-ends and as a
//  starting point for machine-checked metatheoretic work.
//
//  Typeset with Typst (≥ 0.14). Compile with `make spec` from within the
//  repository's `nix develop` shell.
// ============================================================================

#import "preamble.typ": *

#show: setup-document

// ---------------------------------------------------------------------------
// Cover page
// ---------------------------------------------------------------------------

#set page(numbering: none)

#v(5cm)

#align(center)[
    #text(size: 26pt, weight: "bold")[A Formal Specification of SCL]
    #v(0.3em)
    #text(size: 14pt, style: "italic")[the Skyr Configuration Language]
]

#v(2cm)

#align(center)[
    #text(size: 12pt)[The Skyr Project]
    #v(0.2em)
    #text(size: 10pt)[Version 1.0 · Draft]
]

#v(1fr)

#align(center)[
    #text(size: 9pt, fill: rgb(80, 80, 80))[
        This specification documents SCL as implemented by the reference
        compiler `sclc`. When the text and the reference implementation
        diverge, the implementation is considered authoritative; such
        divergences are to be filed as errata against this document.
    ]
]

#pagebreak()

// ---------------------------------------------------------------------------
// Front matter
// ---------------------------------------------------------------------------

#set page(numbering: "i")
#counter(page).update(1)

#heading(level: 1, numbering: none)[Abstract]

SCL — the _Skyr Configuration Language_ — is a statically typed, eagerly
evaluated, purely expression-oriented language used to describe
infrastructure deployments. Its central design goals are _structural
clarity_, _compositional reuse_, and _static safety_ in the presence of
optional values. To that end, SCL combines bidirectional type inference
over a system of structural records, dictionaries, lists and first-class
generic functions with a _propositional type refinement_ mechanism that
narrows types along the success and failure paths of Boolean decisions.

This document gives a rigorous specification of SCL in the usual tradition
of small type-theoretic calculi. After fixing the lexical and abstract
syntax, we develop the metatheory in four layers: a structural type
language with μ-types for recursion, a subtyping relation derived from
the standard record and function rules, a bidirectional type system
augmenting synthesis and checking with propositional side effects, and a
big-step operational semantics. The document closes with a treatment of
the module and package system and an appendix summarising every judgment
form in one place.

#pagebreak()

#heading(level: 1, numbering: none)[Contents]

#outline(title: none, depth: 2)

#pagebreak()

// ---------------------------------------------------------------------------
// Body: restart page numbering to arabic
// ---------------------------------------------------------------------------

#set page(numbering: "1")
#counter(page).update(1)

#include "ch01_introduction.typ"
#include "ch02_lexical.typ"
#include "ch03_syntax.typ"
#include "ch04_types.typ"
#include "ch05_subtyping.typ"
#include "ch06_propositions.typ"
#include "ch07_static.typ"
#include "ch08_dynamic.typ"
#include "ch09_modules.typ"
#include "ch10_stdlib.typ"
#include "appendix.typ"

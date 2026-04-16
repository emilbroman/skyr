// Document preamble: page setup, typography, and semantic helpers
// shared across all chapters of the SCL formal specification.

#import "@preview/curryst:0.5.1": rule, prooftree

// ---------------------------------------------------------------------------
// Global page & font setup
// ---------------------------------------------------------------------------

#let setup-document(body) = {
    set document(title: "A Formal Specification of SCL", author: "The Skyr Project")
    set page(
        paper: "a4",
        margin: (top: 2.5cm, bottom: 2.5cm, left: 2.8cm, right: 2.8cm),
        numbering: "1",
    )
    set par(justify: true, leading: 0.62em, first-line-indent: 0pt)
    set text(font: ("New Computer Modern", "Libertinus Serif", "Linux Libertine"), size: 10.5pt, lang: "en")
    show raw: set text(font: ("JetBrains Mono", "Menlo", "DejaVu Sans Mono"), size: 9.3pt)
    show heading.where(level: 1): it => {
        pagebreak(weak: true)
        v(0.4em)
        set text(size: 20pt, weight: "bold")
        block(it)
        v(0.6em)
    }
    show heading.where(level: 2): it => {
        v(0.4em)
        set text(size: 13pt, weight: "bold")
        block(it)
        v(0.1em)
    }
    show heading.where(level: 3): it => {
        v(0.2em)
        set text(size: 11pt, weight: "bold", style: "italic")
        block(it)
    }
    set heading(numbering: "1.1")
    body
}

// ---------------------------------------------------------------------------
// Mathematical / semantic macros
// ---------------------------------------------------------------------------

// Meta-variables and judgmental forms appear so often that centralising
// them yields consistent typography and makes global restyling trivial.

// The turnstile used for typing judgments; a thin space after is nicer.
#let ts = sym.tack.r

// "Has type" / "synthesizes" / "checks" arrows, inspired by bidirectional
// typing literature (Dunfield & Krishnaswami 2021, Pierce & Turner 2000).
#let synth = sym.arrow.r.double       // e ⇒ A (synthesize)
#let check = sym.arrow.l.double       // e ⇐ A (check against)
#let reduces = sym.arrow.b             // big-step: e ⇓ v
#let raises = sym.arrow.t              // big-step: e ⇑ exc
#let subtype = sym.lt.tilde            // ⪍/≲ — subtype / assignable
#let asgn = math.class("relation", $<:$)   // <: — declared upper bound

// Semantic brackets for the denotation of types / values.
#let sem(x) = $lr(⟦ #x ⟧)$
#let env = $Gamma$
#let eenv = $rho$                      // evaluation environment

// Propositional refinement judgments.
#let proven = sym.tack.r
#let refines = sym.arrow.long

// Disjointness of types (no shared inhabitants under subtyping).
#let disjoint = sym.hash

// Logical implication (used in propositional refinement).
#let implies = sym.arrow.r.double
#let iff = sym.arrow.l.r.double

// ---------------------------------------------------------------------------
// Nonterminal, terminal, and keyword rendering
// ---------------------------------------------------------------------------

// In EBNF, nonterminals are set in italics; terminals / keywords in
// sans-serif monospace. We keep the distinction visible throughout.
#let nt(name) = text(style: "italic", name)
#let kw(s) = raw(s)
#let lit(s) = raw(s)

// ---------------------------------------------------------------------------
// Theorem-like environments
// ---------------------------------------------------------------------------

#let _boxed(name, body, fill: rgb(245, 247, 251), stroke: rgb(190, 200, 220)) = {
    block(
        width: 100%,
        fill: fill,
        stroke: (left: 2pt + stroke),
        inset: (left: 10pt, right: 10pt, top: 8pt, bottom: 8pt),
        spacing: 0.9em,
    )[
        #strong(name) #h(0.3em) #body
    ]
}

#let remark(body) = _boxed("Remark.", body, fill: rgb(248, 248, 248), stroke: rgb(170, 170, 170))
#let note(title, body) = _boxed(title + ".", body, fill: rgb(250, 244, 232), stroke: rgb(200, 170, 110))
#let definition(name, body) = {
    let counter-state = counter("definition")
    counter-state.step()
    context [
        #_boxed(
            "Definition " + str(counter-state.get().first()) + " (" + name + ").",
            body,
        )
    ]
}

// A figure-like block for a single inference rule with a descriptive name.
#let rulefig(name, body) = figure(
    body,
    caption: name,
    kind: "rule",
    supplement: [Rule],
)

// ---------------------------------------------------------------------------
// Grammar tables
// ---------------------------------------------------------------------------

// A two-column grammar row: left side is a nonterminal; right side is
// its production. Use a bullet on continuation lines.
#let grammar(..rows) = {
    let entries = rows.pos()
    table(
        columns: (auto, 1fr),
        stroke: none,
        align: (right + top, left + top),
        inset: (x: 6pt, y: 4pt),
        ..entries.map(r => {
            let (lhs, rhs) = r
            (
                [#lhs #h(0.4em) ::=],
                rhs,
            )
        }).flatten()
    )
}

// Alternative production continuation: `| something`
#let alt(x) = [$|$ #h(0.4em) #x]

// Horizontal layout for a set of inference rules on one page.
#let ruleset(..rules) = {
    let entries = rules.pos()
    grid(
        columns: 1fr,
        gutter: 1.2em,
        ..entries
    )
}

// A judgment box — renders a typing judgment as a centered framed display.
#let judgment(body) = {
    align(center)[
        #box(
            stroke: 0.5pt,
            inset: (x: 8pt, y: 4pt),
            radius: 2pt,
            body,
        )
    ]
}

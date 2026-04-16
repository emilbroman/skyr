// ============================================================================
//  Chapter 2 — Lexical structure
// ============================================================================

#import "preamble.typ": *

= Lexical Structure

An SCL source file is a sequence of Unicode characters, grouped by the
lexer into a sequence of _tokens_ (Section 2.2). This grouping is almost
entirely context-free: the sole exception is the interaction between
string interpolation and matching braces, described in Section 2.5. We
present the grammar of tokens in the extended Backus–Naur form of
Section 2.1, with a deliberate bias toward the concrete tokens produced
by the reference lexer.

== Source representation

An SCL source is a finite sequence of extended grapheme clusters in the
Unicode Text Segmentation sense (UAX #29). The lexer advances one
grapheme cluster at a time; since all significant tokens are composed of
ASCII characters, most of this machinery is invisible to the grammar and
manifests only in the precise column counting used for diagnostics.

A source _position_ is a pair of a 1-based line number and a 1-based
column number; a _span_ is a pair of positions marking a half-open range.
Tokens carry a span that locates them in the source; we elide spans from
the grammar below, recovering them implicitly from the concatenation of
constituent tokens.

The reference implementation imposes an upper bound of 1 MiB on source
file size; files that exceed this limit are rejected before tokenisation
begins. This spec treats any such rejection as indistinguishable from a
syntax error.

== Token classes

Tokens are partitioned into the following classes:

+ _Punctuators and delimiters_: `{`, `}`, `#`, `:`, `,`, `.`, `(`, `)`,
  `[`, `]`, `=`, `==`, `;`, `/`, `+`, `-`, `*`, `!`, `!=`, `<`, `<:`,
  `<=`, `>`, `>=`, `&&`, `||`, `?`, `??`.
+ _Reserved keywords_: `import`, `let`, `fn`, `export`, `extern`,
  `if`, `else`, `for`, `in`, `nil`, `true`, `false`, `exception`,
  `raise`, `try`, `catch`, `type`, `as`.
+ _Identifiers_: sequences of alphabetic, numeric, and underscore
  graphemes, starting with an alphabetic or underscore grapheme, that
  are not reserved keywords.
+ _Integer literals_: either the single digit `0` or a nonempty sequence
  of decimal digits whose first digit is nonzero.
+ _Float literals_: two integer-literal productions separated by a
  single `.`.
+ _String literals_: delimited by `"` and (optionally) containing
  interpolation segments delimited by `{` and `}` (Section 2.5).
+ _Trivia_: whitespace, line comments beginning with `//` and doc
  comments beginning with `///`. Trivia is recognised by the lexer and
  then discarded before the parser runs.

A _reserved keyword_ is _not_ an identifier even if it matches the
identifier production: the lexer classifies `let` as `LetKeyword` rather
than `Symbol("let")`, and so on for each entry in the keyword list.

== Keywords

#align(center)[
    #table(
        columns: 6,
        stroke: 0.4pt,
        inset: (x: 6pt, y: 4pt),
        [`import`], [`let`], [`fn`], [`export`], [`extern`], [`if`],
        [`else`], [`for`], [`in`], [`nil`], [`true`], [`false`],
        [`exception`], [`raise`], [`try`], [`catch`], [`type`], [`as`],
    )
]

Every keyword above is strictly reserved: no identifier may collide with
one. The list is frozen for any given language version.

== Numeric literals

Integer literals match either the singleton `0` or a nonempty sequence
of decimal digits beginning with a nonzero digit. The productions
#raw("01"), #raw("007") and similar are therefore malformed and are
reported as `Unknown` tokens by the lexer.

A float literal is lexed only when a dot and at least one further digit
immediately follow an integer literal. Thus `3.14` is a single float
literal but `3.` alone is not a float; it is parsed as the integer `3`
followed by the dot token. Similarly `.5` is not a float literal; it is
a dot followed by the integer `5`. The single-token lookahead required
for this decision is captured in the lexer routine
`peek_is_float_fraction_start`.

== String literals and interpolation

A string literal is delimited by ASCII double quotes. Within it, the
escape sequences in Table 1 are recognised; all other occurrences of
`\` together with the following grapheme are retained verbatim.

#figure(
    align(center)[
        #table(
            columns: 2,
            stroke: 0.4pt,
            inset: (x: 6pt, y: 4pt),
            align: (center, left),
            [*Escape*], [*Replacement*],
            [`\n`], [the newline character `U+000A`],
            [`\r`], [the carriage-return character `U+000D`],
            [`\t`], [the tab character `U+0009`],
            [`\\`], [a single backslash `\`],
            [`\"`], [a literal double quote, not closing the literal],
            [`\{`], [a literal open brace, suppressing interpolation],
        )
    ],
    caption: [Recognised escape sequences inside string literals.],
)

Interpolation is introduced by an unescaped `{` within a string literal.
On encountering such a brace, the lexer pushes an _interpolation_ state
recording the depth of matched braces within the embedded expression,
emits a `StrBegin` token whose body is the prefix consumed so far, and
switches to scanning the embedded expression as if it were ordinary
source code. Nested braces inside the embedded expression increment and
decrement the depth counter; only when the depth returns to zero does
the lexer treat the next `}` as closing the interpolation and return to
string scanning, emitting either a `StrCont` token (if a subsequent
`{` begins a further interpolation segment) or a `StrEnd` token (if a
closing `"` is found).

This design deliberately avoids the common pitfall of scanning the
embedded expression as a single lookahead: record literals and dict
literals inside an interpolation are lexed normally because their
braces merely modify the depth counter.

A _simple_ string literal — one containing no unescaped `{` — is lexed
as a single `StrSimple` token. String interpolation is a _surface
syntax_: the parser elaborates a sequence of `StrBegin`, intermediate
expressions, `StrCont`, further expressions and finally `StrEnd` into
an `Interp` AST node.

== Trivia

Trivia consists of ASCII and Unicode whitespace, line comments and doc
comments. Line comments begin with `//` and extend to the next line
terminator; doc comments begin with `///` and extend similarly. Doc
comments associated with an immediately following record field, type
declaration, or `let` binding are retained by the parser as _doc
strings_ on the corresponding AST node; they play no role in the
static or dynamic semantics and are relevant only to external tools.

No trivia tokens appear in any grammar rule of Chapter 3.

== Lookahead conventions

The parser is LL-like and performs only finite lookahead over tokens
(after trivia removal). There are two noteworthy lookahead rules:

+ An open square bracket `[` is treated as the beginning of an indexed
  access _only_ when the immediately preceding token ends at the same
  source position. A whitespace-separated `[` is parsed as the start
  of a list literal. This rule is captured in the `adjacent_open_square`
  predicate of the reference implementation and resolves the
  ambiguity between `xs [i]` (list followed by an indexed access on
  itself) and `xs[i]` (indexed access on `xs`).
+ A postfix `<` in expression position tentatively introduces type
  arguments at a call site; if the ensuing production fails to yield a
  parenthesised argument list, the parser backtracks and treats the
  token as the less-than operator instead.

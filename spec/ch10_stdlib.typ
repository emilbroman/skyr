// ============================================================================
//  Chapter 10 — Extern functions
// ============================================================================

#import "preamble.typ": *

= Extern Functions

SCL's extern mechanism is the single bridge between the language and
its host runtime. An extern expression binds a declared type to a
host-supplied implementation; at evaluation time, the host's dispatch
function supplies values for extern invocations. This chapter
specifies the extern mechanism itself and its run-time plug-in
protocol for resource externs. Any collection of externs exposed as a
_standard library_ (conventionally the #raw("Std") package) is a
_userland_ artefact above this mechanism: the language does not
mandate any particular extern be present, and a host is free to
provide, extend, or replace the set of externs it dispatches.

== Extern functions

An _extern function_ is a value whose implementation is supplied by
the host evaluator rather than by an SCL expression. Syntactically, an
extern expression has the form

$ #kw("extern") s colon T $

binding a host-supplied implementation named by the string literal
$s$ to a declared type $T$:

#align(center)[
    #raw("export let square = extern \"math/square\": fn(Int) Int")
]

The string argument to #kw("extern") is a _dispatch key_ that names
the host implementation, and the trailing type annotation is
_mandatory_ — there is no inference of the host's signature. Extern
values are ordinary first-class values and may be passed around,
stored in records, and invoked indirectly.

At evaluation time, an extern invocation consults a fixed partial
function

$ "host" : "Name" times overline("Value") arrow "Value" $

supplied by the deployer. The reference implementation's host
dispatches pure externs synchronously and resource externs via the
run-time plug-in protocol (RTP), as described in Section 10.3.

== Pure versus resource externs

Externs divide into two disjoint classes:

- _Pure externs_ are deterministic functions of their arguments. The
  host is obliged to return the same value on identical inputs; no
  side effects are observable.
- _Resource externs_ are the elimination form of _resources_ — units
  of deployable state. A resource extern returns a pending value
  during initial evaluation; the deployer reconciles the pending
  value against the real world by invoking the corresponding plug-in,
  and re-evaluates the downstream graph with the resolved output.

The type system does not distinguish pure and resource externs: both
have ordinary function types. The distinction is carried entirely by
the host's dispatch behaviour.

== The run-time plug-in protocol

A resource extern delegates its create, update, and delete operations
to an external process that implements the _run-time plug-in
protocol_ (RTP). The protocol is a small bidirectional RPC over a
Unix socket or TCP connection; the salient operations are:

- _plan_: given the resource's declared inputs and, optionally, the
  previously observed state, return either a concrete output record
  or a _plan_ that describes the change to be applied.
- _apply_: given a plan, perform the real-world change and return the
  resulting output record.
- _delete_: given a previously observed state, destroy the real-world
  resource and return success.

For the purposes of this specification we treat the host as providing
a single mathematical function $"planner"$ mapping (resource-name,
inputs) pairs to outputs or $bot$, leaving the internal mechanics of
the protocol opaque; see the plug-in documentation for operational
details.

== Summary

The extern mechanism is a tightly typed façade over a host-provided
environment. The language itself is agnostic to which externs are
registered: a host with no externs is still able to compile and
evaluate every non-#kw("extern") program. The choice of what externs
to provide determines what programs can _do_ at run time, and it is
precisely this choice that distinguishes the SCL compiler from the
SCL deployer. A conventional standard library — a package named
#raw("Std") in the reference implementation — is one such choice and
is documented outside this specification.

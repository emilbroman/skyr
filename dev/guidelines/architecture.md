# Architecture

Guidelines for reasoning about and extending Skyr's overall architecture.

## Horizontal Scalability is the Organizing Principle

Skyr is designed to scale horizontally. Every component must have — or have a clear path to — an implementation that can be deployed freely in dozens or even hundreds of replicas. This is the lens through which all other architectural decisions should be evaluated.

To keep a design of this shape legible, think of every component in terms of four things:

- **Contract** — what it accepts and produces.
- **Responsibilities** — what it owns.
- **Limits of responsibility** — what it explicitly does *not* do.
- **Synchronization points** — where it hands off to another system.

Example: the DE (deployment engine) is responsible for compiling and executing SCL code. It is *not* responsible for state reconciliation. That responsibility is handed off to the RTE via a queue.

## Coherence Through Redundancy and Idempotency

In the presence of instability across both sides of a handoff, coherence is maintained by two principles working together:

- **Redundant emission** — a component can emit however many messages it wants.
- **Idempotent consumption** — the consumer must process potentially-conflicting messages in an order that allows it to drop messages that no longer make sense.

The DE→RTE handoff is the load-bearing example: the DE emits transition requests freely, and the RTE collapses or discards stale ones based on current desired state.

## Push for Writes, Pull for Reads

The fundamental rule for handoffs between components:

- **Writes are pushed**, redundantly and idempotently.
- **Reads are pulled**.

Whether the push is a queue message or a gRPC call matters less than having a clear story for what happens when a call fails. On crash:

- The failure must result in a *pushed* report somewhere in the system, so that a new attempt is made — either directly by the caller, or implicitly via the redundancy/idempotency mechanism elsewhere.

Concrete example: if the RTE fails to satisfy a resource transition because the RTP call to the plugin fails, the reconciliation loop will implicitly emit another transition request (if it is still desired). The RTE itself does not need to track the retry — the system as a whole does.

When the synchronization point is a database rather than a transport, see [Storage](storage.md) — first-class handoff storage is a sync point with the same coherence requirements. The decision of where a boundary lives (library vs service) is covered in [Boundaries](boundaries.md).

## Queue vs gRPC

Rule of thumb for choosing the shape of a write handoff:

- **Queue** when there is a clean push-based handoff between *known* Skyr components. Examples:
  - DE → RTQ → RTE
  - DE/RTE → RQ → RE
  - RE → NQ → NE

- **gRPC** when there is a higher-level *protocol* at play — i.e., when the two sides may be on different versions, or when one side is "any plugin" or "any node". Examples:
  - **RTP** sits between the RTE and *any plugin*.
  - **SCOP** sits between the container orchestrator (plugin) and *any live SCOC node*, which may be on a different version.

  Compatibility evolution is easier to track with gRPC's schema discipline than with raw queue messages.

- **gRPC even when a queue is tempting** when a higher-level protocol is involved. In that case, place a consumer/caller component between the queue and the protocol boundary, giving a topology of:

  ```
  producer → queue → consumer/caller → receiver
  ```

  This preserves the push-based handoff for redundancy/idempotency, while keeping the protocol boundary on a versioned, schema-tracked transport.

## When Adding a New Component

Before introducing a new service, queue, or protocol, work through:

1. What is the **contract** — what does it accept and produce?
2. What are its **responsibilities**, and what is explicitly *outside* them?
3. What are the **synchronization points** with neighboring components?
4. For each handoff: is this a write (push) or a read (pull)?
5. For each push: is the right transport a queue or gRPC, by the rule of thumb above?
6. For each handoff: what happens on crash? What pushes the retry?

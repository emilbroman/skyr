# Boundaries

Guidelines for where boundaries live in Skyr, and how they relate to data ownership and the service/library distinction.

See also: [Architecture](architecture.md), [Storage](storage.md).

## Boundaries Live in Code, Not Infrastructure

When a piece of functionality or a database client is wrapped in a library, **that is a boundary** — every bit as much as a service running across the network is a boundary.

A library introduces a strictly typed protocol around whatever it is doing, regardless of whether data ever needs to be serialized. Whether the consumer of the library is in the same process or in a different replica is, from the perspective of the contract, irrelevant.

## The Majestic Monolith Pattern

The defining test for a well-designed library boundary in Skyr: **can it be extracted into a standalone service, leaving a client to that service in its place, without changing the external API?**

```
[Service A]              [Service B]
   [lib X]                  [lib X]
        \\\\\         //////
            [database Y]
```

(lib X wraps a client to database Y)

should be refactorable to:

```
[Service A]              [Service B]
   [lib X]                  [lib X]
        \\\\\         //////
              [Service X]
                  |
             [database Y]
```

If that refactor would force changes to the API of `lib X` as seen by services A and B, the boundary was drawn wrong.

## The Important Decision is the Library, Not the Service

Introducing the **logical boundary** (the library) is the decision that matters. Promoting that library to a physical service later is a smaller, mechanical step that callers should not need to notice.

Practical consequence: do not skip the library on the grounds that "we're not going to extract this into a service anyway." The library is paying its way as a typed protocol and as a unit of ownership, regardless of whether it ever becomes its own process.

## Architectural Roles Are Communication Boundaries Enshrined as Components

Following from the [Architecture](architecture.md) principle of loosely coupled components: we introduce **architectural roles** (like the RTQ) to abstract over the *communication boundary* between components, by wrapping that boundary in a library and treating it as a component in its own right.

Today the RTQ is a library wrapping a RabbitMQ client. In the future, it might make sense to extract a service that wraps the RTQ. Either form is consistent with the same architectural role — that is the point.

## Data Ownership

Data is owned by **a crate** — whether that crate is a library or a standalone service is incidental.

Concrete consequences:

- "Who owns CDB?" is answered by the `cdb` crate, not by which daemon happens to call into it.
- If two services both depend on `cdb`, they are both clients of the same owner — they are not co-owners.
- Reaching around the owning crate to talk to the database directly is a violation of the boundary, regardless of whether the owner is currently a library or a service.
- If a new use case needs data that crosses two owning crates, it goes through both crates' APIs. It does not join across them. (See [Storage: No Cross-Logical-Database Joins](storage.md#no-cross-logical-database-joins).)

## Practical Checklist

When introducing a new piece of shared functionality or shared state, ask:

1. Is there a typed contract I can write down? (If not, the boundary isn't ready.)
2. Could I extract this into a service later without breaking callers?
3. Which crate owns this data or capability? (Exactly one answer.)
4. If two services need access, are they both going through that owning crate's API?

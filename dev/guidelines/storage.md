# Storage

Guidelines for using databases and queues in Skyr, and for choosing storage solutions for new development.

See also: [Architecture](architecture.md) — particularly the "synchronization points" framing, since first-class storage in Skyr usually *is* a synchronization point.

## Two Modes of Storage

Storage is used in Skyr in one of two distinct ways. Decide which up front, and make the choice clear architecturally.

### 1. Component-Owned Storage

A single component uses a database as private persistence — for its own state, scratch data, internal coordination, etc. The node registry in Redis is an example.

This mode is rare. When you reach for it:

- The boundary must be **clear architecturally** — it should be obvious from the design that this storage is owned by one component.
- The boundary must **not be violated** — no other service is allowed to query this database. If another service ends up needing to read it, that's a signal the storage should be promoted to a first-class handoff point (mode 2) with a proper lib crate and contract.

### 2. First-Class Handoff Points

Storage is used to coordinate or communicate between services. CDB, SDB, UDB, RDB, LDB, ADB are all examples. This is the common case.

For this mode, the rules are strict:

**a) Wrap the database client in a dedicated lib crate with a strict API.**

The crate is the contract. Direct database access from outside the lib crate is not allowed. The strictness of the API is what keeps the handoff coherent across many replicas of producers and consumers.

This is the [Boundaries](boundaries.md) principle in action — the lib crate is a code-level boundary that could later be extracted into its own service without changing the API its callers see.

**b) Writes must be race-free.**

The database is acting as a synchronization point, so it must be protected against race conditions — including TOCTOU. Enforcement is the responsibility of the lib crate. The exact mechanism varies by backing store (LWT in ScyllaDB, transactions/WATCH in Redis, conditional writes in S3, etc.), but the lib crate's API must be designed so that callers cannot accidentally introduce a race.

## No Cross-Logical-Database Joins

A single underlying instance of a database may host multiple logical databases (e.g., one ScyllaDB cluster backing both CDB and RDB). In such cases:

- **Joins across logical databases are not allowed**, even when the underlying engine could technically support them.
- Each logical database is owned by its lib crate, and lib crates do not reach into each other's tables.

If you find yourself wanting to join across two lib crates, the right move is to have the relevant component read from each and combine in application code — or to reconsider whether the schema is split correctly.

## Choosing Storage for New Development

- **Nothing is off-limits.** Pick the best tool for the job.
- **Don't add new solutions when something in the current arsenal suffices.** If Redis is already in the system, don't add memcached. If ScyllaDB is already there, don't add Postgres for a similar workload.

The current arsenal (as of writing): ScyllaDB, Redis, RabbitMQ, Kafka/Redpanda, S3/MinIO. Reach for a new tool only when none of these is a reasonable fit.

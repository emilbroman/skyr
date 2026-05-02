# Skyr Global Directory Database (GDDB)

GDDB is a library that wraps a ScyllaDB client and exposes a typed API for the global name directory: org names and repo names mapped to their home Skyr region.

## Role in the Architecture

GDDB is the authoritative store for **case-insensitive name reservation** and **home-region routing**. Every org and repo name in Skyr is reserved in GDDB at creation time, keyed on `sha1(lower(name))`; every read path that resolves an org or repo name consults GDDB first to determine which region serves it.

```
API → GDDB ← SCS
```

GDDB is global by virtue of ScyllaDB, not by virtue of being a service. There is no GDDB binary, no gRPC boundary, no replication daemon — just a keyspace and this library.

## Capabilities

### Name Reservation

- `reserve_org(&OrgId, &RegionId)` — atomically claim an org name for a region.
- `reserve_repo(&RepoQid, &RegionId)` — atomically claim an `org/repo` name for a region.

Reservation uses ScyllaDB lightweight transactions (`INSERT IF NOT EXISTS`) so two simultaneous creates of `Foo` and `foo` race deterministically; one wins, one gets `NameTaken`.

### Lookup

- `lookup_org(&OrgId)` — returns the home region of an org, or `None` if not reserved.
- `lookup_repo(&RepoQid)` — returns the home region of an `org/repo`, or `None` if not reserved.

### Hashing

Names are hashed with `ids::name_hash`, which is `sha1(name.to_ascii_lowercase())`. The hash is the table's primary key; it gives case-insensitive uniqueness without leaking the namespace for enumeration. The original-cased name is stored alongside for ops/audit purposes.

## Schema

GDDB manages its own keyspace and table creation. The keyspace currently uses `SimpleStrategy` for parity with the rest of the codebase and the dev ScyllaDB single-node setup; in production, operators are expected to `ALTER KEYSPACE gddb` to `NetworkTopologyStrategy` with replicas in every region before bringing up region #2. The library's `IF NOT EXISTS` keyspace creation will then be a no-op, and writes route via Scylla's normal multi-DC replication.

## Failure Model

Creation paths run **GDDB-first, then UDB/CDB**. If the second step fails, the GDDB reservation is left in place — orphan reservations are accepted as a permanent name claim. Compensating deletes are deliberately avoided because they risk violating the case-insensitive uniqueness invariant under retry races.

## Related Crates

- [IDs](../ids/) — typed identifiers, including the `name_hash` helper.
- [auth_token](../auth_token/) — defines the identity-token format; verifying keys are now served by [IAS](../ias/) per region rather than published into GDDB.
- [API](../api/) — reserves names on `createOrganization` / `createRepository` / `signup`; consults GDDB on every org/repo lookup.
- [SCS](../scs/) — consults GDDB on every SSH path resolution.
- [IAS](../ias/) — wraps UDB; consulted via DNS once GDDB has resolved a name to its home region.
- [UDB](../udb/) — stores org/user records (region-implicit); accessed only through IAS.
- [CDB](../cdb/) — stores repo records (region-implicit).

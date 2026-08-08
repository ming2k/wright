# ADR-0030: Single Database for System State

## Status

Accepted

## Context

[ADR-0005](0005-two-database-design.md) mandated two SQLite databases:
`installed.db` for the live system state and `archives.db` as a catalogue of
locally built archives. The split was motivated by resolver performance,
corruption isolation, and the idea that `archives.db` could one day serve as
a repository index.

In practice the archive catalogue duplicated information that already lives
in the archives themselves: every `.wright.tar.zst` in `parts_dir` carries a
`.PARTINFO` manifest with full metadata. A separate `archives.db` could drift
from the real contents of `parts_dir` (archives copied, pruned, or sealed
out-of-band), and its "repository index" portability rationale conflicted
with the local-inventory model settled later (see
[ADR-0023](0023-parts-as-maintenance-ledger.md): parts are maintenance-ledger
artifacts, not distribution products). The repository has shipped a single
database for a long time: `archives.db` was removed and `installed.db` was
renamed to `/var/lib/wright/wright.db`, but ADR-0005 was never formally
retired.

## Decision

Wright keeps exactly one database per target root: `wright.db` (default
`/var/lib/wright/wright.db`, overridable with `general.db_path` or `--db`),
owned by the `wright-state` crate. It holds installed state, file ownership,
advisory dependency edges, transactions, and history.

The local archive inventory has no database. It is derived on demand by
scanning `parts_dir` and reading `.PARTINFO` metadata from the archives.

This ADR supersedes [ADR-0005](0005-two-database-design.md).

## Alternatives

### Keep two databases

A catalogue that re-states archive metadata must be reconciled against
`parts_dir` on every read anyway, so it adds a consistency problem without
adding information. Rejected.

### No database at all

Installed state, ownership, transactions, and history need atomic,
queryable persistence; flat files would re-invent SQLite badly. Rejected.

### Repository-style index

Treating `archives.db` as a shareable index assumes parts move between
machines. ADR-0023 settles that they do not; plans move, parts stay local.
Rejected.

## Consequences

- One transaction domain: deploy, upgrade, and remove need no cross-database
  coordination, and crash recovery replays a single WAL.
- Archive lookups read archive metadata on demand — slightly slower than an
  indexed query, but always exact and never stale.
- Pruning or hand-editing `parts_dir` cannot corrupt system state; the worst
  case is a missing archive at merge time, reported as an ordinary error.
- `wright launch` initialises the target's own `wright.db` inside `--root`;
  there is no second database to mirror.

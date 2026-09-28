---
id: ADR-0043
title: "The registry is a derived index; maintenance measures, snapshots, rebuilds, and reclaims"
status: proposed
date: 2026-09-23
---

# ADR-0043: The registry is a derived index; maintenance measures, snapshots, rebuilds, and reclaims

## Status

Proposed

## Context

Wright runs for years on one machine. Over that lifetime three different
failure modes accumulate, and they are usually reported as one symptom:
"storage usage is high, the system needs a big cleanup." They are not one
problem, and treating them as one produces a deletion command that cannot be
made safe.

The first mode is **ownership drift**. Files land on the live root that no
deployed part recorded: a hook that writes outside its staging tree, a user
who edits a config in place and leaves the original behind, a `make install`
run by hand, a part whose removal deleted only what `files` listed. Every
verification surface Wright has runs in one direction — registry to disk.
`check --files`, `doctor`, and `lint --verify` all start from
`db.list_parts()`, read `db.get_files(part.id)`, and stat each recorded path.
They detect deletion and modification. Nothing walks `root_dir`, so nothing
detects addition. `owner <FILE>` is the only unowned-path signal, and it takes
required positional arguments: the operator must already know the path.
`list --filter orphan` reports dependency-graph orphans, never filesystem
orphans. An earlier `prune --untracked` existed and was removed; the removal
was correct for reasons recorded under Alternatives.

The second mode is **registry damage**. A destructive update, a failed
migration, or a corrupted database file leaves `wright.db` unreadable, and
`InstalledDb::open` is on the path of every command that answers a question:
`list`, `files`, `owner`, `check`, `doctor`, `history`, `graph`, `plan`. The
one command most worth running in that situation — `doctor` — dies with the
rest. `crash_recover` in `src/cli/common.rs` swallows the open failure
(`if let Ok(db) = …`) and the command then fails with a generic
"failed to open database" context. Migrations are forward-only; a newer schema
than the binary produces "database schema v{current} is newer than this
binary; please upgrade wright" with no downgrade or repair path. There is no
snapshot before migration, no `PRAGMA foreign_key_check`, and
`docs/reference/database-design.md`'s Migration System table has five rows —
migration files, tracker, initialization, upgrade, immutable history — with no
row for backup, rollback, or repair.

The third mode is **monotone accumulation**. Of the six locations where
Wright accumulates artifacts, three are reachable by `wright clean`
(`parts_dir`, `forge_dir`, `logs_dir`) and three are not:

- `store_dir`. ADR-0019 recorded the gap: "CAS store is append-only and never
  garbage-collected. A future feature should address CAS garbage collection."
  `CasStore::remove` exists and is marked `#[allow(dead_code)]` with no
  caller. The cost is structural, not incidental: a CAS fingerprint is
  `sha256(build_key + every dependency fingerprint)`, so any change anywhere
  in a closure mints new entries, while `docs/how-to/maintain-os-parts.md`
  prescribes pessimistic cascading rebuilds (`--rdeps=all`). Every rolling
  update therefore leaves a whole superseded closure in the store, and the
  guide that causes this never mentions `clean`.
- `source_dir`. `docs/explanation/checkpoint-recovery.md` states that an
  updated tarball is kept twice, "both old and new versions under different
  CAS paths". ADR-0038 leaves orphaned bare repositories that "can be deleted
  manually".
- `ledger_dir`. ADR-0041 states "the ledger grows monotonically;
  retention/pruning policy is deliberately deferred", and
  `docs/how-to/estimate-build-costs.md` states "nothing rotates or prunes the
  ledger".

No command reports bytes. `clean --dry-run` prints counts only —
"{} workspace(s), {} archive(s), {} log entry(s)" — so the operator cannot see
which of the six locations is responsible before choosing a flag, and cannot
verify afterwards that space was actually returned. Migration V6 dropped
`parts.install_size` with the rationale "computed on demand", but no query
computes it; `files.file_size` is populated on every deploy by
`transaction/fs.rs` and never aggregated.

The three modes share one root cause. ADR-0019 already stated the principle
that resolves it: "A database record saying 'forge completed' can
desynchronise from reality… The file system is the only invariant-proof source
of truth." Wright applies that principle to build and deploy — CAS plus the
delivery WAL — and stops there. It has never been extended to the live root or
to `wright.db` itself. The registry is treated as authoritative state when it
is in fact an index over facts that live elsewhere: on the live root, and in
`.PARTINFO`, `.FILELIST`, `.PLANSRC`, and `.BUILDINFO` inside every archive in
`parts_dir`.

## Decision

### The registry is a derived index

`wright.db` records what the inventory and the live root already contain. It
is authoritative for nothing that cannot be re-derived: part identity,
versions, file ownership, dependencies, conflicts, replaces, and shadowing all
come from archives that carry `.PARTINFO` and `.FILELIST`. This ADR takes over
the deferred work in ADR-0019 (CAS garbage collection) and ADR-0041 (ledger
retention) on that basis.

Two consequences are binding on every future change:

1. Every mutating operation must leave the registry rebuildable from
   `parts_dir`. A fact that exists only in `wright.db` is a fact that a
   rebuild destroys, so such facts require an explicit decision and a stated
   recovery story.
2. `doctor` and `clean` must remain usable when the registry is unreadable.
   `clean` already is: `src/cli/clean.rs` runs on configuration alone and
   never calls `open_db`. `doctor` must gain the same degraded path rather
   than fail on `ctx.open_db()`.

### `wright storage` measures before anything deletes

A read-only command reports, per location — `parts_dir`, `store_dir`,
`source_dir`, `forge_dir`, `logs_dir`, `ledger_dir`, `wright.db` and its
`-wal`, plus the `history` row count — the bytes, the entry count, and the
retention rule under which the entry would be reclaimed. The existing
`dir_stats` helper in the build ledger already returns `(bytes, files)` and is
reused rather than reimplemented. Deployed footprint comes from aggregating
`files.file_size`, which is what V6 meant by "computed on demand".

Discipline follows from the command: **a location with no `storage` row gets no
deletion flag.** Retention predicates are set against measured bytes, not
estimates.

### Snapshots precede migrations

`run_migrations` takes a `VACUUM INTO` snapshot whenever `pending` is
non-empty, named to carry both versions
(`<db>.pre-migrate-v<from>-to-v<to>.bak`), and the migration-failure error
names that file and the rebuild command. A failed destructive update then
costs a restore instead of a system.

Because connections run `journal_mode = WAL` with `synchronous = NORMAL`,
snapshotting a live database requires either `wal_checkpoint(TRUNCATE)` first
or the `-wal` and `-shm` files alongside the copy. `VACUUM INTO` writes a
consistent standalone file and is therefore the mechanism; hand-copying
`wright.db` is documented as unsafe without checkpointing.

Integrity checking adds `PRAGMA foreign_key_check` beside the existing
`PRAGMA integrity_check`, so referential damage from a partial migration is
detected rather than silently queried.

The same mechanism is exposed on demand as `wright doctor --snapshot`, for operators
who want a restore point before a risky action rather than only before a schema
change. Automatic snapshots are bounded by an explicit retention rule so that
the recovery mechanism does not itself become an unbounded accumulation.
Snapshots are restored via `wright doctor --restore <BACKUP>`.

### `wright doctor --repair` rebuilds the registry

Scanning `parts_dir` — and, with `--from-store`, orphaned copies in
`store_dir` — and reading each archive's `.PARTINFO` and `.FILELIST`
reconstructs `parts`, `files`, `dependencies`, `conflicts`, `replaces`, and
`shadowed_files`. `parts_dir` is the default source because its placement is
the canonical inventory; the store is a fingerprint-named cache whose
namespace is shared across plans and eras, so it is opt-in. Identity comes
from `.PARTINFO`, matching how `clean --stale` and the part store already
resolve archives, so flat archives sealed before ADR-0034 reindex too.

`history` cannot be derived and is not. It is restored from a snapshot when
one exists; otherwise the loss is reported explicitly rather than hidden.
Repair is explicit, never an implicit step, and never runs inside another
command (ADR-0004).

### `clean` gains the three missing locations

`--store`, `--sources`, and `--ledger` extend the single disk-reclamation
command established by ADR-0040. Each carries its own explicit predicate; the
three never share one flag:

- `--store` removes a CAS entry whose fingerprint is neither reachable from
  the currently installed closure nor the latest version of its
  `(plan, output)` pair.
- `--sources` removes by age or last use, and by absence from every current
  plan's declared sources, including ADR-0038's orphaned bare repositories.
- `--ledger` rotates per plan, keeping the most recent N build records and
  garbage-collecting `_detached/` snapshots.

Deletion reports bytes actually reclaimed. This matters specifically for
`store_dir`, where entries are hard links into `parts_dir` (falling back to
copy only on `EXDEV`): unlinking one entry releases nothing while another
link survives. Apparent size and reclaimed size are therefore reported
separately, with reclaim counted only where `nlink == 1`.

Every deletion stays previewable with `-n`/`--dry-run` and every dry run
prints byte totals. ADR-0040's safety model is unchanged: state-changing
commands execute by default and preview on request, because everything `clean`
deletes remains rebuildable.

### `wright doctor --drift` reports drift and never deletes

A read-only mode (`wright doctor --drift`, alias `--audit`) walks the managed scope, subtracts the `files` path set,
and reports the residue as `unowned-in-scope`, completing the three-way
verification alongside the existing `owned-but-modified` (`lint --verify`) and
`owned-but-missing` (`check --files`). `--json` makes it usable as a CI gate.

The managed scope is the FHS whitelist `crates/wright-part/src/fhs.rs`
already enforces at seal time — `/usr/{bin,lib,lib64,share,include,libexec,
libdata}` plus `/etc`, `/var`, `/opt`, `/boot`. Reusing that single
definition keeps the seal-time contract and the audit-time contract identical
instead of maintaining two lists that drift. Wright does not own `/`, and
audit makes no claim about paths outside the scope.

Audit is a precondition for any later adoption path, not a deletion path.
`unowned` never implies `safe`.

### External parts must declare their paths before audit can run

`provide_part` inserts one `parts` row with `origin = 'external'` and no
`files` rows. On any system bootstrapped with `wright provide`, the entire
foreign toolchain would report as unowned, which makes audit unusable on
exactly the layered systems that need it. Audit therefore treats external
parts as an explicit blind spot: it reports them as unverified coverage rather
than reporting their paths as drift. Closing the gap requires external parts
to carry a path list, which is a separate decision.

## Alternatives

### A single `wright gc` that fixes everything

One command that prunes all six locations, compacts the database, and removes
unowned files would be convenient and would be wrong. Its three jobs need
three different safety models: caches are freely deletable, the registry needs
a snapshot before it is touched, and unowned files may be load-bearing. A
single verb hides which model applied. ADR-0040 consolidated deletion into
`clean` precisely so that one surface carries one safety model; a catch-all
`gc` reverses that. Rejected.

### Restore `prune --untracked` with deletion

The flag was removed for good reason and must not return in deleting form.
Unowned files include paths hooks created on purpose, configs a user edited,
and files a foreign part installed without registering. Deleting on the
`unowned` predicate destroys user data on the first run. The correct
escalation is report, then an explicit human decision, then either adoption —
recording the path against a part so `remove` will clean it — or quarantine.
Never unlink on the predicate alone. Rejected.

### Automatic GC on a schedule or as an install side-effect

Background or post-install cleanup violates ADR-0004: Wright does not perform
implicit actions the operator did not ask for. It also destroys the property
that makes `clean` safe to run — knowing exactly which invocation deleted
what. Rejected.

### Truncate the `history` table under retention

ADR-0023 makes the audit trail a primary purpose of the inventory. Silent
truncation removes the evidence that justifies keeping archives at all. Any
history retention must be an explicit, separately-spelled opt-in naming the
cutoff, and is out of scope here. Rejected.

### Rebuild the registry by rescanning the live root

Attractive because it needs no archives, but it cannot recover part identity.
A file at `/usr/lib/libz.so.1` carries no plan name, no version, and no
dependency set. Reverse-engineering ownership from the filesystem yields a
registry with no provenance, which is worse than the damaged one it replaced.
Reindex reads `.PARTINFO` because that is where identity actually lives.
Rejected.

### Introduce a `reports_dir` for cost and usage data

ADR-0041 rejected this and the rejection still holds: a per-plan append-only
ledger gives cost estimation its series data natively, and a second storage
location for the same facts adds a consistency obligation for no gain. Usage
is computed on demand from what already exists. Rejected.

### Repair by deleting `wright.db` and re-running `launch`

Works only when every plan is still present, every source is still fetchable,
and the machine can afford a full rebuild. It is the right last resort on a
system being provisioned and the wrong answer on a live one with a damaged
database and years of build time behind it. `reindex` exists so that this
remains a choice rather than the only path. Rejected as the default.

## Consequences

### Positive

- The three accumulated gaps stop being open questions: ADR-0019's "a future
  feature should address CAS garbage collection" and ADR-0041's deferred
  retention both acquire an owner and a predicate.
- A damaged registry stops being a single point of failure. `doctor` and
  `clean` keep working, snapshots make a failed migration recoverable, and
  `reindex` restores ownership facts from the inventory without rebuilding
  anything.
- Storage questions become answerable in one command instead of six `du`
  invocations, and retention decisions get made against measured bytes.
- Verification becomes three-way. Wright can finally say what is on the live
  root that it does not know about, which is the precondition for every other
  drift story.
- `CasStore::remove` stops being dead code.

### Negative

- Maintenance capabilities (`storage`, `doctor --snapshot/--restore`,
  `doctor --repair`, `doctor --drift`) and three new `clean` flags widen the CLI
  surface without introducing leaky top-level database abstractions. ADR-0032's
  consistency conventions apply to all of them.
- `reindex` cannot restore `history`, `installed_at` timestamps, or
  origin distinctions between `build`, `manual`, and `dependency`. A rebuilt
  registry is honest about parts and files and lossy about provenance; it must
  say so in its output rather than presenting itself as equivalent.
- Reindexing from `store_dir` inherits CAS ambiguity: the fingerprint
  namespace is shared across plans and eras, which is why `CasStore::store`
  already replaces mismatched entries. Archives in `parts_dir` are the
  preferred source and `store_dir` stays opt-in.
- `audit` on a large root is a full walk, and its output on a layered system
  is dominated by the external-part blind spot until that is closed.
- Snapshots consume disk on every schema migration, which is why the Decision
  bounds them by retention; an unbounded recovery mechanism would become the
  accumulation problem it exists to solve.
- Reporting reclaimed bytes separately from apparent bytes adds output
  complexity, but silently reporting the wrong number is worse.

### Migration

No schema change is required by this ADR. Implementation order follows risk:
`usage` and the pre-migration snapshot first, since both are read-only or
additive; `db reindex` and the degraded `doctor` path next, since they carry
the architectural claim; `clean`'s three flags after that; `audit` last, since
its value depends on the external-part decision.

`docs/reference/cli-reference.md`, `docs/reference/database-design.md`,
`docs/reference/local-inventory.md`, and `docs/reference/configuration.md`
change in the same pull request as the behaviour they describe, per the
hot-data synchronisation rule. A how-to for maintaining Wright's own on-disk
state is added beside them: `docs/how-to/maintain-os-parts.md` explicitly
disclaims that subject, and today no document covers it.

---
id: ADR-0057
title: "Transactional Cold Purge: a Cache Trust Boundary, Atomic Cleanup, and Exact Accounting"
status: accepted
date: 2026-10-27
scope: cli, core/foundry, config
superseded_by: null
negative_knowledge: true
---

# 0057. Transactional Cold Purge: a Cache Trust Boundary, Atomic Cleanup, and Exact Accounting

- Status: Accepted
- Date: 2026-10-27
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

ADR-0056 introduced `--no-cache` as "purge `source_dir` and `store_dir`, then
rebuild". It got the operator-facing intent right and the implementation wrong
in five ways:

1. **Deletion was not transactional.** Entries were unlinked one by one. An I/O
   failure or a crash left a partially purged cache; a later ordinary build
   could combine the old CAS with a new source tree.
2. **It re-downloaded proven content.** An `http` source with a concrete
   `sha256` is content-addressed: if the cached bytes hash to the declaration,
   they are the right bytes. Purging them and downloading identical bytes is
   not caution, it is waste.
3. **It lied about bytes.** The reported figure was apparent size. CAS entries
   are hard links to `parts_dir` archives, so unlinking them often frees
   nothing; nested roots double-counted.
4. **The purge could delete non-cache data.** If `source_dir` contained
   `parts_dir`, the database, the ledger, or the forge workspace, enumerating
   the source root deleted them too.
5. **The lock story had holes.** `doctor --repair --from-store` read the CAS
   without the command lock; `launch` took no command lock at all; `clean`
   could race a build.

ADR-0056's "discard the caches that could have caused the error" remains the
right philosophy. The refinement is to distinguish **what can be proven** from
**what cannot**, and to make the destructive part a transaction.

---

## Decision Drivers

- **A cache that cannot prove its content must not survive.** From this run nor
  from the next one.
- **A cache that can prove its content is an input, not a cache.** Re-downloading
  a verified pinned tarball adds no guarantee.
- **Destructive operations are transactions.** Staging + commit + recovery, with
  every intermediate state safe for a concurrent or subsequent build.
- **Measure what you actually free.** Inode-aware accounting, not apparent size.
- **Never delete through a path you do not own.** Overlapping storage layouts are
  rejected before any command runs.
- **One lock hierarchy.** `wright` → `build` → `db`.

---

## Decision Outcome

### 1. Cache trust boundary

| Category | Examples | Cold-mode behavior |
|---|---|---|
| Verified input | `http` with concrete `sha256`; cached bytes match | **kept and reused** |
| Verified input, corrupt | same, hash mismatch | deleted, re-downloaded |
| Unverifiable input | git named-ref snapshot, `http` `SKIP` | deleted, re-fetched |
| Reproducible input | local source cache copy (re-copied every fetch) | deleted |
| Derived state | charge marker, checkpoints, staging cache, layers/base, CAS | never read; CAS purged |
| Inventory | `parts_dir` | never deleted |

A cold build therefore means: **do not trust unverifiable caches or derived
state; keep inputs whose content is proven.** `charge` performs the matching
verification: a pinned artefact is reused when its bytes still hash to the
declaration, and re-downloaded when they do not. `SKIP` is always re-fetched in
cold mode; git named refs always re-resolve.

### 2. Transactional purge

The purge is staged per root under `<root>/.wright-purge-<txid>/`, journaled at
`<db parent>/purge/<txid>.json`, and committed:

1. Plan the selection and compute exact reclaimed bytes (`--dry-run` stops here).
2. Write the journal in `staging` state.
3. Stage roots **store first, source second**, renaming each selected entry
   (relative paths preserved). Any failure rolls every staged entry back
   immediately.
4. Write the journal in `committed` state.
5. Delete the staging directories. Leftovers are reaped by recovery.

Recovery (`cache_purge::recover`) is idempotent:

- `staging` → restore staged entries. If a destination already exists, the
  **current state wins** and the staged copy is discarded.
- `committed` → delete the staging directories.
- Unjournaled `.wright-purge-*` residue is treated as committed.
- Unreadable cache roots are skipped with a warning: recovery is best-effort
  cleanup and must not fail commands that do not touch those roots. A purge
  itself still validates its roots and fails closed.

Recovery runs before cache decisions in `install`, `upgrade`, `build
--no-cache`, `launch`, cache-cleaning `clean`, and `doctor --repair`. A normal
`build` does not hold the `wright` lock and does not run recovery; the
store-first ordering makes that window safe.

**Why store first:** if the process dies after staging the CAS but before the
source, the visible state is "CAS empty, source full/partial", which can only
cause a rebuild or a re-fetch. The reverse order — source gone, CAS intact —
could let an old frozen artifact answer for fresh source bytes. The ordering
removes that window by construction.

### 3. Exact accounting

`reclaimed_bytes(entries, parts_dir)` walks the selected entries, counts each
inode once, and excludes inodes still referenced from `parts_dir`. The purge
preview and `clean --store` report this figure. Apparent size is no longer
reported as reclaimed.

### 4. Storage layout validation

`GlobalConfig::validate_storage_layout(db_path)` rejects any layout where
`source_dir` or `store_dir`:

- overlaps the other cache root in either direction,
- overlaps `parts_dir`, `db_path`, `ledger_dir`, `logs_dir`, or `build.forge_dir`
  in either direction, or
- contains `plans_dir`, `folios_dir`, `executors_dir`, or any
  `extra_plans_dirs`.

Paths are canonicalised when they exist, so symlinks cannot disguise an overlap.
`load` runs the check; `cache_purge::plan` runs it again with the effective
`--db`/launch-redirected paths. A violation is a startup error, not a warning.

### 5. Lock hierarchy

Fixed order: **`wright` → `build` → `db`**.

| Command | Locks |
|---|---|
| `install` / `upgrade` | `wright` → (`--no-cache`: `build`) → `db` |
| `build` | `--no-cache`: `wright` → `build`; otherwise `build` |
| `clean` (not `-n`) | `wright` → `build` |
| `launch` (not `-n`) | `wright` → (`--no-cache`: `build`) → `db` |
| `doctor --repair` (not `-n`), `doctor --restore` | `wright` → `db` |
| Read-class commands | none (ADR-0044) |
| `clean -n`, `doctor --repair -n`, `doctor --snapshot` | none |

`crash_recover` at dispatch acquires and releases the `db` lock before the
command lock; that startup exception is unchanged and documented.

`clean --dry-run` and mutating doctor previews are lock-free because they mutate
nothing. Advisory locks cannot stop external `rm`/`cp`; that remains out of
scope.

### 6. Surface

- `--no-cache` is exposed on `build`, `install`, `upgrade`, and now `launch`.
  `launch` purges the **target root's** caches; host caches are untouched.
- `package` and `merge` do not expose it: they consume staged trees/archives and
  never read source or CAS.

---

## Invariants

- **`[INV-COLD-01]` Explicit only.** No config key, no environment default, no
  implicit trigger.
- **`[INV-COLD-02]` Only pure caches.** A purge may delete `source_dir` and
  `store_dir` entries. `parts_dir`, the registry, the ledger, and logs MUST NOT
  be deleted.
- **`[INV-COLD-03]` Invalidate derived state before inputs.** The CAS is staged
  before the source cache. No intermediate state may let an old CAS entry serve
  new source bytes.
- **`[INV-COLD-04]` Verified inputs are not caches.** A pinned `http` entry whose
  bytes match its declared `sha256` and is required by the selected build set is
  kept and reused. Everything unverifiable is discarded.
- **`[INV-COLD-05]` Purges are transactions.** Staging → commit, with an
  idempotent journal; an interrupted purge is either fully restored or fully
  finalized. Current state wins on rollback conflicts.
- **`[INV-COLD-06]` Reclaimed means reclaimed.** Byte reports are inode-aware
  and exclude data still live in `parts_dir`.
- **`[INV-COLD-07]` One lock order.** `wright` → `build` → `db`; mutating
  commands that touch caches or the registry participate; Read commands do not.
- **`[INV-COLD-08]` No overlapping storage.** A cache root that overlaps
  inventory, the registry, the ledger, logs, the forge workspace, or the other
  cache root is rejected before any command runs.

---

## Rejected Alternatives & Negative Knowledge

### 1. Keep bypass-only (never delete)
- **Why considered**: It cannot destroy anything.
- **Why rejected**: It is a split-brain cache — untrusted this run, trusted the
  next. A poisoned entry survives to reproduce the failure. The user-facing
  intent is discard, and the transaction makes discard safe.

### 2. Delete verified pinned inputs too
- **Why considered**: Literal "no cache".
- **Why rejected**: A declared SHA-256 proves the bytes. Re-downloading a
  multi-GB tarball to obtain identical content is not extra safety. Corrupt
  files still fail verification and are discarded.

### 3. Per-plan scoped purge
- **Why considered**: Avoid invalidating unrelated plans.
- **Why rejected**: The source cache has no reliable plan dimension (`as`
  aliases may be shared), and CAS keys are closure fingerprints, not plan
  identities. A scoped purge is incomplete and creates false precision. The
  keep set is scoped only where proof exists: the selected build set's pinned
  inputs.

### 4. Epoch / blue-green cache namespaces
- **Why considered**: Never delete the old cache; switch to a fresh namespace.
- **Why rejected**: It doubles disk during a build, requires promotion/merge for
  flat cache filenames, and the old namespace still needs a discard decision.
  Only justified if re-acquisition becomes prohibitively expensive; then it
  should be designed as a whole, not bolted onto this mode.

### 5. Per-entry unlink without a journal
- **Why considered**: Simpler; caches are re-fetchable.
- **Why rejected**: A crash could leave the dangerous combination (old CAS,
  missing source) and the command would report a failure after partially
  destroying state. Staging + journal costs one rename per entry and removes
  the class.

### 6. Let recovery skip the `wright` lock
- **Why considered**: Recovery is just cleanup.
- **Why rejected**: It moves files in the caches. It must serialize with purges,
  builds, and installs; the lock hierarchy is cheap.

### 7. Protect against external processes
- **Why considered**: The caches are directories; anything can `rm` them.
- **Why rejected**: Advisory `flock` is the project's model (ADR-0044). The
  contract is "mutate through wright". Documented, not defended.

---

## Consequences

### Positive

- A cold run either fully discards the unverifiable caches or leaves them as
  they were; a crash cannot produce a half-purged state.
- Verified inputs survive, so cold rebuilds do not re-download immutable
  tarballs.
- Byte reports match what the kernel frees.
- A misconfigured storage layout is rejected before it can delete the inventory
  or the database.
- `launch` and `doctor --repair` participate in the same lock order as installs.

### Negative Consequences & Trade-offs

- **Breaking**: overlapping storage layouts are now startup errors.
- **Breaking**: `--no-cache` keeps verified pinned inputs instead of
  re-downloading them.
- A new private journal directory (`<db parent>/purge/`) and hidden staging
  directories (`.wright-purge-*`) exist during a purge and after a crash until
  recovery.
- The keep set hashes the selected build set's pinned cache files (bounded by
  those files, not the whole cache).
- Exact accounting walks the selected entries; a purge is I/O-bound anyway.
- The purge is still global for unverifiable entries: other plans' git
  snapshots, `SKIP` entries, and CAS entries are discarded.

---

## Links

- Supersedes: [ADR-0056](0056-explicit-cold-builds.md)
- Related ADRs: [ADR-0004](0004-no-magic-behavior.md),
  [ADR-0040](0040-clean-prune-consolidation-and-fresh-flag.md),
  [ADR-0043](0043-registry-as-derived-index-and-maintenance-surface.md),
  [ADR-0044](0044-command-privilege-classes-and-read-only-query-path.md),
  [ADR-0054](0054-content-addressed-source-identity.md)
- Implementation:
  `crates/wright-config/src/lib.rs` (`validate_storage_layout`),
  `crates/wright-actions/src/operations/cache_purge.rs`,
  `crates/wright-actions/src/operations/clean.rs`,
  `crates/wright-actions/src/foundry/charge/{mod,fetch}.rs`,
  `crates/wright-actions/src/operations/{install/execute,build,upgrade,launch,doctor}.rs`,
  `src/cli/{build,install,upgrade,launch,clean,doctor,mod}.rs`
- Narrative: [Build Mechanics](../reference/build-mechanics.md),
  [Plan Build](../explanation/plan-build.md),
  [Install Design](../explanation/install-design.md),
  [Configuration](../reference/configuration.md)

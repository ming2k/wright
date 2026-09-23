# How to Reclaim Disk Space and Recover the Registry

This guide covers maintaining Wright's own on-disk state: the six locations it
accumulates over a machine's lifetime, and what to do when a destructive update
or a failed migration damages the registry. It is the operator-facing companion
to [ADR-0043](../adr/0043-registry-as-derived-index-and-maintenance-surface.md).

Scope:
- This document is about Wright's own storage and database.
- It is not about maintaining your distribution's plan tree; see
  [Maintain OS parts](maintain-os-parts.md) for that.

## 1. Measure before you delete

Start with `wright storage`. It reports the byte size, file count, and
reclamation rule for every location Wright owns:

```bash
wright storage
```

```
LOCATION          BYTES    FILES  RULE
forge            2.1 GiB     1204  clean (build workspaces)
parts           18.4 GiB       96  clean --stale (superseded versions)
store           11.2 GiB       88  clean --store (entries unlinked from any archive)
sources          3.0 GiB       41  clean --sources [--older-than-days N]
logs            14.2 MiB       63  clean --logs (command logs)
ledger          88.1 MiB      510  clean --ledger [--keep-builds N]
database         4.1 MiB        1  doctor --snapshot / doctor --repair
total           34.8 GiB
deployed        19.9 GiB  (14523 files, deployed footprint)
```

Every row names the command that reclaims it, and the rule runs both ways: a
location with no `storage` row has no deletion flag. Do not guess at a flag's
worth — read its row first.

## 2. Reclaim, one location at a time

Each `clean` flag owns exactly one location and one predicate. Combine them
freely, and always preview first with `-n`:

```bash
wright clean -n                        # workspaces only (the default)
wright clean --stale -n                # superseded archive versions
wright clean --store -n                # orphaned CAS cache copies
wright clean --sources --older-than-days 30 -n
wright clean --ledger --keep-builds 5 --keep-snapshots 5 -n
wright clean --logs -n
```

The dry run prints the exact removal set **and the byte total**, so you can
confirm the space is real before committing.

### What each location costs you, and why it accumulates

- **`--store` (CAS).** A part's cache fingerprint is
  `sha256(build_key + every dependency fingerprint)`. Any change anywhere in a
  closure mints new entries, and pessimistic cascading rebuilds
  ([Maintain OS parts](maintain-os-parts.md)) change whole closures at a time.
  Every rolling update therefore leaves a superseded closure in the store. This
  is the largest and least obvious accumulator.

  A CAS entry is a **hard link** to the part archive it caches. Removing a
  still-linked entry frees *nothing* — the archive keeps the inode alive — so
  `--store` skips those and reports only bytes actually reclaimed. If `df` does
  not move as much as the dry run promised, that is the reason, and the numbers
  are already telling you the truth.

- **`--sources`.** The source cache. A tarball that gets a new version is kept
  twice, under two cache keys. `--older-than-days N` bounds removal by age;
  without it, every cached source is removed (it is re-fetched on the next
  build).

- **`--ledger`.** The per-plan audit ledger is append-only by design. Rotation
  is opt-in: `--keep-builds N` keeps the newest N build records per plan and
  `--keep-snapshots N` the newest N plan-source snapshots. This is the only
  `clean` flag that *rewrites* rather than deletes, and it never touches
  `wright.db`'s `history` table.

- **`--archives` / `--stale`.** `--archives` removes every archive of the named
  plans; `--stale` removes only superseded versions. Neither is a cache — an
  archive is inventory, rollback, and audit. Prefer `--stale`.

## 3. When the registry is damaged

A failed migration or a corrupted database file leaves `wright.db` unreadable.
`wright.db` is a **derived index**: part identity, file ownership,
dependencies, conflicts, and replaces all live in the archives'
`.PARTINFO`/`.FILELIST`. So the registry is rebuildable, and the `history` table
is the only thing that is not.

**First, check whether `doctor` still runs.** It degrades rather than dying
with the database: it keeps scanning the archive closure, reports the
registry-dependent checks as skipped, and exits non-zero with a pointer:

```bash
wright doctor
```

### Recover from a pre-migration snapshot

Wright snapshots the database with `VACUUM INTO` before applying any pending
migration. Snapshots are named `<db>.pre-migrate-v<from>-to-v<to>.bak` and the
three most recent are kept:

```bash
ls /var/lib/wright/wright.db.pre-migrate-*.bak
wright doctor --restore /var/lib/wright/wright.db.pre-migrate-v19-to-v20.bak
```

`VACUUM INTO` is the only safe way to copy a database running in WAL mode — a
plain `cp` can miss pages committed to the `-wal` sidecar. `--restore` validates
the snapshot's header and removes the `-wal`/`-shm` sidecars so the restored
main file is authoritative.

### Rebuild the registry from the inventory

When there is no usable snapshot — or the damage predates this feature —
rebuild from the archives:

```bash
wright doctor --repair -n     # preview
wright doctor --repair        # rebuild
wright doctor                 # confirm
```

`--repair` reads every archive's `.PARTINFO` and `.FILELIST`, stats each recorded
path against the live root, and replaces the registry in a single transaction.
It never touches `history`. What it cannot re-derive it reports rather than
fakes: `installed_at` timestamps are left NULL, and a part's original `origin`
is preserved only from a best-effort read of the still-openable database.

Take a manual snapshot before a risky action with `wright doctor --snapshot`, which uses
the same `VACUUM INTO` mechanism.

## 4. Audit for files Wright does not know about

`wright doctor --drift` walks the managed scope — the same FHS whitelist the seal step
enforces — and reports files that no deployed part owns. It is the mirror of
`check --files` (which finds owned files that went missing) and
`lint --verify` (which finds owned files that changed):

```bash
wright doctor --drift
wright doctor --drift --json       # CI gate
```

It is **read-only**, and that is deliberate: `unowned` does not mean `safe`.
The residue includes paths a hook created on purpose, configs a user edited, and
files a foreign part installed. Review each path and decide — record it against
a part, or leave it — but never delete on the predicate alone.

Externally provided parts (`wright provide`) record no file paths, so drift audit
cannot tell their content from genuine drift. When any are present, it names
them in an `external_blind_spot` statement rather than reporting their files as
drift.

## See also

- [ADR-0043](../adr/0043-registry-as-derived-index-and-maintenance-surface.md) —
  the design decision behind all of the above
- [CLI reference](../reference/cli-reference.md) — every flag in full
- [Local part inventory](../reference/local-inventory.md) — what the archives carry
- [Database design](../reference/database-design.md) — schema, migration, recovery

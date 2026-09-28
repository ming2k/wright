---
id: ADR-0044
title: "Command privilege classes and a genuinely read-only query path"
status: proposed
date: 2026-09-23
---

# ADR-0044: Command privilege classes and a genuinely read-only query path

## Status

Proposed

## Context

Wright is a system package manager on a single machine, yet it has no declared
privilege model. No command inspects its effective user. The only `uid` read in
the codebase selects XDG directories in `config.rs` (`default_general`,
`get_xdg_config`) and has nothing to do with authorization. Whether a command
succeeds is decided entirely by the file modes of `/var/lib/wright`,
`/var/log/wright`, `/var/tmp/wright`, and `/etc/wright`. The permission
contract is emergent, undocumented, and untestable.

`docs/reference/cli-reference.md` states that the Query & Inspection group is
"read-only introspection." That is false. Every query command except `lint`
opens the installed-state database through `ctx.open_db()` →
`InstalledDb::open()`:

- `InstalledDb::open` calls `create_dir_all` on the database's parent
  (`crates/wright-state/src/database/core.rs:87`), so a query creates
  directories.
- It acquires an **exclusive** advisory lock (`core.rs:95`; the identity is
  locked with `LockMode::Exclusive` at `core.rs:62`).
- It starts the read-write writer thread, which opens the file for writing
  (`core.rs:106`), applies mutating PRAGMAs through `configure_connection`
  (`core.rs:114`), and runs schema migrations through `schema::init_db`
  (`core.rs:126`).
- Every command context then calls `crash_recover` (`src/cli/common.rs:100`),
  which runs `recover_if_needed` — a mutating routine that marks `PLANNING`
  deliveries rolled back, resets mid-flight operations, and cleans up
  completed transactions.

Three symptoms follow, all reproduced on a live install:

1. **Queries write.** `wright list --root /tmp/empty` created
   `/tmp/empty/var/lib/wright/wright.db` and a lock file out of nothing.
   Against a database holding a `PLANNING` delivery, `wright list` deleted the
   transaction row. A read command performed persistent mutation, which
   ADR-0004 ("no implicit magic behavior") forbids.
2. **Queries block.** `LockMode::Shared` exists and maps to `LOCK_SH`
   (`crates/wright-state/src/lock.rs:67`) but is referenced only from tests;
   production always locks exclusively. Measured on a live install: `wright
   list` takes ~20 ms; while another process held the database lock the same
   command blocked 11.5 s and 24.6 s, releasing only when the holder exited,
   and would fail with `LockError` past the 30 s budget. Two concurrent `list`
   invocations still serialized (76 ms). ADR-0042's stated consequence that
   "readers never wait on the writer" holds only within one process.
3. **An unprivileged user can stall the package manager.** With
   `/var/lib/wright/lock` not root-only writable, and `flock` being
   uid-agnostic, any user able to open `db-wright.db.lock` can hold `LOCK_EX`
   and make a privileged `install`/`upgrade` wait out its timeout and fail.

When the schema is behind, a non-privileged query hits the same wall the code
already anticipates: the failure text `attempt to write a readonly database` is
preserved as a regression fixture in `util/logging.rs:380`. Only the lock path
converts `EACCES` into a typed error carrying a remediation hint
(`WrightError::AccessDenied`, `crates/wright-state/src/error.rs:16`); database
open, migration, and directory creation failures surface as opaque
`DatabaseError` context.

This meets the ADR significance test on three counts: reversal is expensive (it
touches `wright-state`, every CLI dispatcher, tests, and user documentation), it
alters a security posture and a storage-access boundary, and it generates
binding invariants. ADR-0043 independently found the same read-path coupling
(`InstalledDb::open` on the path of every command that answers a question) and
the same `crash_recover` failure-swallowing in `src/cli/common.rs`; this ADR
fixes the access boundary that ADR-0043 works around.

## Decision

### 1. Every subcommand declares exactly one privilege class

| Class | Semantics | Commands |
| :--- | :--- | :--- |
| **Read** | Unprivileged-safe. Mutates no persistent state and creates no paths. | `list` `files` `owner` `history` `plan` `graph` `check` `doctor` `lint` |
| **Local** | Writes only the forge workspace, part store, source cache, and logs. Rootless when those paths are user-owned. | `resolve` `build` `package` `clean` `prune` |
| **System** | Mutates the live target root and/or the installed-state database. Requires privilege. | `install` `upgrade` `remove` `merge` `provide` `launch` |

The class is data, not prose: it is encoded once, in the CLI layer, and every
dispatcher reads it. A new subcommand is added to exactly one arm of the
dispatcher's match, where its class determines whether recovery runs and which
database handle it receives.

### 2. The Read class is genuinely read-only

Read commands open the database through a distinct `ReadOnlyDb::open_read_only()`
path that has none of the mutation side effects of `InstalledDb::open`:

- no `create_dir_all`; a missing database is an error, never materialized;
- `SQLITE_OPEN_READ_ONLY` with `query_only`; no writer thread is spawned;
- no mutating PRAGMAs — `journal_mode` and its siblings belong to the writer
  path alone;
- no migrations. If `PRAGMA user_version` is behind `CURRENT_DB_VERSION`, the
  command fails with a typed error naming the command that migrates;
- no crash recovery.

The read-only guarantee is enforced by the *type system*, not by discipline.
`ReadOnlyDb` is the base type carrying every query method and `read()`;
`InstalledDb` is a distinct type that owns the writer actor, the exclusive
lock, and `write()`, and derefs to `ReadOnlyDb`. A query call site holding a
`&ReadOnlyDb` therefore cannot name `write()` at all — the mutation surface is
unreachable, so a read path that mutates is a compile error rather than a code
review finding.

### 3. Readers take no process lock; mutations take an exclusive lock

`InstalledDb::open` (the mutation path) keeps `LockMode::Exclusive`, which
prevents two writers — including two migrations — from racing. Read commands
take **no** process lock.

This is deliberate and is the crux of the fix. `flock` shared locks still wait
on an exclusive holder, so a reader that took `LockMode::Shared` would block for
the entire duration of a build or install — exactly the behaviour being removed.
Read consistency instead comes from SQLite WAL snapshot isolation: each
`ReadOnlyDb::read` opens its own read-only connection and observes a stable
committed snapshot even while a writer is mid-transaction. The schema-version
probe at open time is the one guard against a reader seeing a half-migrated
database. The result is that a long build no longer blocks `wright list`,
`owner`, or the `--web` graph server, and concurrent readers proceed in
parallel — which is what ADR-0042 already claims but did not deliver across
processes.

### 4. Lock ownership is privileged

The lock directory and lock files are created owned by the privileged user and
are not writable by unprivileged users, so no unprivileged process can hold the
lock a System command needs. Lock creation reports a typed `AccessDenied`
rather than a generic failure when the directory is not writable.

### 5. Privilege failures are typed and actionable

Every `EACCES`/`EROFS` on a system path — database open, migration, directory
creation, lock — maps to `WrightError::AccessDenied` with a remediation hint.
Raw SQLite text such as "attempt to write a readonly database" never reaches
the user.

### 6. The class is part of the public contract

`docs/reference/cli-reference.md` states each command's class, and no document
claims "read-only" for a command that writes.

### Invariants and behavioral boundaries

- **`[INV-PRIV-01]` Read purity**: A Read command performs zero writes to the
  installed-state database, creates no files or directories, and acquires no
  process lock. Enforced structurally by the `ReadOnlyDb` / `InstalledDb` type
  split; verified by running each Read command against a database holding a
  mid-flight delivery and asserting the rows are unchanged.
- **`[INV-PRIV-02]` No implicit creation**: A Read command never creates a
  database. A missing database is a typed error naming the path. (Directly
  enforces ADR-0004.)
- **`[INV-PRIV-03]` Privileged recovery**: Only System commands run migrations
  or crash recovery. No Read or Local command calls `recover_if_needed` or
  `init_db`.
- **`[INV-PRIV-04]` Non-blocking readers**: Readers acquire no process lock and
  do not wait on an exclusive writer; only the mutation path acquires
  `LockMode::Exclusive`, and it is the only place that lock appears.
- **`[INV-PRIV-05]` Lock isolation**: No unprivileged process can hold or
  block the lock required by a System command.
- **`[INV-PRIV-06]` Typed privilege errors**: Permission failures on system
  paths surface as `AccessDenied` with a remediation hint; no raw SQLite
  read-only text is user-visible.

## Alternatives

### Document the privilege requirements without changing code

Rejected. Documentation alone would ratify "Query & Inspection is read-only"
while `list` deletes delivery rows and creates databases. A false invariant is
worse than an absent one, because it is the artifact future contributors and AI
assistants trust.

### Gate every command on effective-uid zero

Rejected. It forecloses rootless deployment into a user-owned `--root` — the
`Local` class's whole purpose — and denies unprivileged users the introspection
(`list`, `owner`, `history`) they legitimately need on a shared machine.

### Readers acquire a *shared* lock

Rejected. A shared `flock` still waits while an exclusive holder is active, so a
reader would block for the whole duration of a build or install — reintroducing
the very contention this ADR removes (measured: 11.5 s and 24.6 s stalls). WAL
snapshot isolation plus the schema-version probe gives read consistency without
any lock.

### A single `open(path, read_write: bool)`

Rejected as the sole mechanism. The hazard is the mutating *steps*
(`create_dir_all`, migrations, PRAGMAs, recovery), not the flag's value, and a
boolean parameter invites a query call site that passes `true`. A separate
`ReadOnlyDb` type — the base that `InstalledDb` derefs to, so `write()` is
simply not nameable through it — makes the mistake uncompilable; this is why the
boolean alone is not chosen.

### Raise or tune the 30-second lock timeout

Rejected. It tunes the symptom rather than removing the contention, and it
lengthens the window an unprivileged holder can stall a privileged command.

## Consequences

Positive:

- Queries become safe, fast, and correct for unprivileged users, and the
  documented "read-only" claim becomes true.
- ADR-0042's read-concurrency consequence now holds across processes, not only
  within one. Measured: a read that took 11.5–24.6 s while another process held
  the lock now completes in tens of milliseconds.
- A read command can no longer create state or silently roll back a crashed
  delivery — the two defects that made the old behaviour unsafe.
- A new class of local denial of service against the package manager is closed.
- Rootless `Local` workflows (build into a user-owned tree, inspect a foreign
  `--root`) become expressible rather than accidental.
- Privilege failures become actionable instead of raw SQLite text.

Negative and trade-offs:

- Two open paths must be kept in sync. Mitigation: they share the same
  reader-connection helper and the same `ReadOnlyDb` base, so the only
  divergence is the writer-only prologue, and read methods physically live on
  the shared base rather than being duplicated.
- A Read command against a schema older than the binary now errors instead of
  silently limping. Mitigation: the typed error names the command that
  migrates, so the fix is one command.
- Crash recovery no longer runs opportunistically on every query; it runs on
  the next System-class command. This is intended — queries must not mutate —
  but it means a crashed delivery stays visible to `history` until then.
- `wright list` (and every Read command) now fails on a database that does not
  exist rather than creating one. Scripts that relied on the implicit
  initialization must run an install (or `wright doctor --snapshot`) first; the error
  says so.
- Where system paths are root-owned, `Read` commands cannot read privileged
  files (`check`, `doctor`, `lint --verify` report `UNREADABLE` rather than
  failing). This is a reporting limitation, not a mutation, and is documented.

### Implementation status

Implemented in this change: the `ReadOnlyDb` / `InstalledDb` type split with
`ReadOnlyDb::open_read_only()`; the read-only path for the nine Read commands;
recovery moved off Read and Local commands onto the System path; typed
`AccessDenied` for database-open and directory-creation failures; and the
`INV-PRIV-01..06` verification above. The lock-ownership baseline in §4
(root-owned lock directory in packaging) remains an operational follow-up, as
does stating per-command classes in `docs/reference/cli-reference.md`.

## Links

- Related ADRs: [ADR-0004](0004-no-magic-behavior.md) (a query creating state
  violates no-magic behavior), [ADR-0042](0042-single-writer-actor-rusqlite.md)
  (single-writer and concurrent-reader architecture this completes),
  [ADR-0030](0030-single-database.md) (single installed-state database),
  [ADR-0043](0043-registry-as-derived-index-and-maintenance-surface.md) (the
  same read-path coupling and `crash_recover` failure-swallowing).
- Related code: `crates/wright-state/src/database/core.rs`,
  `crates/wright-state/src/lock.rs`, `src/cli/common.rs`,
  `docs/reference/cli-reference.md`.

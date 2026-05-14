# ADR-0019: Two-Layer CAS + WAL Recovery for Delivery

## Status

Accepted

## Context

Wright's install/upgrade flow (`wright install`) runs three phases per DAG wave:

1. **Forge** — fetch sources, compile, stage outputs.
2. **Seal** — slice staged outputs, create `.wright.tar.zst` archives.
3. **Deploy** — extract archives into the live root filesystem.

Before this ADR, recovery from a crash during delivery was handled by two
separate mechanisms:

- **Forge checkpoints** (`ForgeCheckpoint`) — per-stage file-system sentinel
  files that allowed skipping already-completed build stages.  These were
  build-pipeline optimisations that never influenced workflow scheduling.
- **Rollback journal** (`RollbackState`) — a JSON-Lines journal that tracked
  filesystem mutations during deploy so that a half-installed package could be
  undone.

This design had several weaknesses:

1. **Forge progress was not transferrable between runs**.  If a build completed
   and the `.wright.tar.zst` was produced, a subsequent `wright install` for
   the same plan at a later time would still re-forge and re-seal everything
   unless the user explicitly used `wright merge` to deploy the pre-existing
   archive.
2. **Deploy crash recovery was per-part, not per-delivery**.  `RollbackState`
   could undo the filesystem mutations of a single part, but it had no concept
   of a multi-part delivery transaction.  If packages A, B, C were being
   installed and the process crashed after A finished but before B started,
   there was no record that B and C still needed to be deployed.
3. **Database tracked build sessions but not delivery sessions**.  The
   `build_sessions` and `execution_sessions` tables (removed) tracked forge
   state in the database, violating the principle that the file system is the
   only source of truth for whether a build artifact exists.

## Decision

We adopt a two-layer recovery architecture inspired by transactional package
management design:

### Layer 1: CAS (Content-Addressed Storage) for Forge + Seal

The file system is the source of truth.  A `.part` file either exists or it
does not.  The database never records whether a plan "was forged successfully".

1. **Fingerprint computation**.  Before forging a plan, compute a closure
   fingerprint:
   ```
   sha256( build_key(plan) + dep₁.fingerprint + dep₂.fingerprint + ... )
   ```
   `build_key` is `Forger::compute_build_key`, which hashes the plan's
   metadata, source URLs/SHAs, and pipeline scripts.  Dependency fingerprints
   are the same closure fingerprints of the plan's direct build dependencies,
   computed recursively from leaves to roots.

2. **CAS store**.  After sealing, hard-link (or copy) the `.wright.tar.zst`
   into `/var/lib/wright/store/<hex>-<name>.part` using the first 16 hex
   characters of the fingerprint as the prefix.

3. **Resume logic**.  When planning a delivery, the resolver queries the CAS
   store for each plan.  If the `.part` exists and is non-empty, the plan's
   forge and seal phases are skipped entirely — the CAS archive is used
   directly for deploy.

4. **Invalidation**.  If a plan's `plan.toml` changes or any of its build
   dependencies change their `plan.toml`, the closure fingerprint changes.
   The old CAS entry is not deleted (it may still be useful for rollback), but
   it is no longer used for new deliveries.

### Layer 2: WAL (Write-Ahead Log) for Deploy

Deploy is the dangerous phase — it mutates `/usr`, `/etc`, and other system
directories.  The database acts as a Write-Ahead Log to ensure the delivery
state machine survives crashes.

Two tables are introduced:

```sql
CREATE TABLE delivery_transactions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    command     TEXT    NOT NULL,
    status      TEXT    NOT NULL DEFAULT 'planning',
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE transaction_ops (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id   INTEGER NOT NULL REFERENCES delivery_transactions(id),
    part_name        TEXT    NOT NULL,
    part_hash        TEXT    NOT NULL,
    action_type      TEXT    NOT NULL,
    execution_order  INTEGER NOT NULL,
    status           TEXT    NOT NULL DEFAULT 'pending',
    old_hash         TEXT,
    error_msg        TEXT
);
```

**State machine (delivery level):**

```
PLANNING → READY → APPLYING → COMPLETED
     ↓         ↓
  ROLLED_BACK (safe — no system mutation has occurred)
```

- **PLANNING**: Resolving dependencies, checking CAS store, populating `transaction_ops`.
- **READY**: All forge+seal complete.  Bullets are loaded.
- **APPLYING**: System is being mutated.  Each op transitions through
  `PENDING → EXTRACTING → HOOKS_RUNNING → DONE`.
- **COMPLETED**: Every op in the delivery succeeded.

**Note**: Once a delivery transaction successfully reaches `COMPLETED` (or is fully `ROLLED_BACK`),
the records in `delivery_transactions` and `transaction_ops` are **deleted** to keep the WAL small.
Audit records of the individual part actions are stored permanently in the `history` table.

**State machine (per-operation):**

```
PENDING → EXTRACTING → HOOKS_RUNNING → DONE
    ↓          ↓            ↓
  FAILED (error recorded, delivery aborted)
```

**Crash recovery (runs at startup of any `wright` command):**

1. Query `delivery_transactions` for rows in `PLANNING`, `READY`, or `APPLYING`.
2. **PLANNING**: Mark as `ROLLED_BACK`.  No system mutation occurred.
3. **READY or APPLYING**:
   - Query `transaction_ops` for the delivery, ordered by `execution_order`.
   - Identify mid-flight ops (`EXTRACTING` or `HOOKS_RUNNING` — their
     filesystem state is undefined).
   - Reset mid-flight ops to `PENDING` so they will be retried.
   - Individual file mutations from the crashed `TransactionContext` are
     already cleaned up by `RollbackState::replay_journal` on `Drop`.
   - Mark the delivery as `ROLLED_BACK` and instruct the user to re-run
     the command.  On re-run, CAS will supply pre-built parts for completed
     forge+seal work, and the deploy phase will resume.
4. If any op is `FAILED`, the recovery aborts — the user must resolve the
   error manually.

### File-system vs Database boundary

| Concern | Source of truth | Rationale |
|---------|----------------|-----------|
| Has a plan been forged? | CAS store (disk) | File existence is not susceptible to DB/disk desynchronisation |
| Has a part been deployed? | `parts` table + `files` table | Must query installed state for conflict checks, dependencies, verification |
| Is a delivery in progress? | `delivery_transactions` + `transaction_ops` | Must survive crashes; SQLite WAL mode provides durability |
| Can a file mutation be undone? | `RollbackState` journal | File-system journal `fsync`-ed before each mutation |

## Consequences

- **Build resume is transparent to the user**.  Running `wright install nginx`
  twice (with no source changes) is a no-op after the first completion
  because all parts exist in CAS.  The third run after a `plan.toml` edit
  only rebuilds the changed plan and its transitive dependents.
- **Crash recovery is automatic**.  The user does not need to pass `--resume`
  or a special flag — any `wright` command triggers the recovery check.
- **CAS store is append-only and never garbage-collected**.  A future feature
  should address CAS garbage collection.
- **The per-part `RollbackState` journal remains active** within
  `TransactionContext` as the lowest-level safety net for filesystem
  mutations, unchanged from before this ADR.

## Related

- ADR-0002 (wave-by-wave install)
- ADR-0005 (two-database design)
- `docs/explanation/delivery-recovery.md`

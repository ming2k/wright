# Delivery Recovery Design

Wright's delivery pipeline (`wright install`, `wright upgrade`, `wright launch`)
runs three phases for each DAG wave: **forge** (build), **seal** (package), and
**deploy** (install to system).  A crash during delivery must not leave the
system corrupted or require the user to manually undo half-finished work.

The recovery architecture has two layers, each with a different source of truth:

| Layer | Phase | Source of truth | Mechanism |
|-------|-------|----------------|-----------|
| 1 | Forge + Seal | File system (CAS store) | Content-addressed `.part` lookup |
| 2 | Deploy | SQLite (WAL) | `delivery_transactions` + `transaction_ops` |

Layer 1 is purely file-system based — if a `.part` archive exists, the build
is done.  Layer 2 uses a Write-Ahead Log in the database because system
mutation (`/usr`, `/etc`) *must* be recoverable after a crash.

**Important**: Once a delivery transaction successfully commits (or rolls back),
the WAL records in Layer 2 are **deleted**. Permanent record-keeping is
handled by the `history` table.

---

## Layer 1: CAS (Content-Addressed Storage)

### Fingerprint computation

Before forging a plan, a *closure fingerprint* is computed:

```
sha256( build_key(plan) + dep₁.fingerprint + dep₂.fingerprint + ... )
```

`build_key` hashes the plan's metadata, source URLs/SHAs, and lifecycle
scripts.  Dependency fingerprints are recursively included so that a change
anywhere in the transitive build tree invalidates all dependents.

### Store layout

After sealing, the `.wright.tar.zst` is hard-linked into
`/var/lib/wright/store/` under the name `<hex_prefix>-<name>.part`:

```
store/
├── a1b2c3d4e5f6a7b8-nginx.part
├── f9e8d7c6b5a43210-openssl.part
└── 1234abcd5678ef90-zlib.part
```

### Resume logic

When `execute_install` plans a delivery, it computes fingerprints for every
plan in the build set, then queries the store.  If a part's CAS entry exists
and is non-empty, the entire forge+seal phase for that plan is skipped.  The
CAS archive is copied to `parts_dir` and treated exactly like a freshly-sealed
part.

### Why file system, not database?

A database record saying "forge completed" can desynchronise from reality.
If the user deletes a `.part` file, a database flag still reads "done", and
the system would attempt to deploy a non-existent archive — breaking
irrecoverably.  The file system is the only invariant-proof source of truth.

---

## Layer 2: WAL (Write-Ahead Log)

Once all forge+seal work is complete, delivery enters the dangerous phase:
mutating the target root filesystem.  The database acts as a Write-Ahead Log.

### Schema

```sql
CREATE TABLE delivery_transactions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    command     TEXT    NOT NULL,    -- "install nginx postgres"
    status      TEXT    NOT NULL,    -- planning | ready | applying | completed | rolled_back
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE transaction_ops (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id   INTEGER NOT NULL REFERENCES delivery_transactions(id),
    part_name        TEXT    NOT NULL,       -- "nginx"
    part_hash        TEXT    NOT NULL,       -- SHA-256 of the .wright.tar.zst
    action_type      TEXT    NOT NULL,       -- install | upgrade | remove
    execution_order  INTEGER NOT NULL,       -- DAG topological order
    status           TEXT    NOT NULL,       -- pending | extracting | hooks_running | done | failed
    old_hash         TEXT,                   -- previous version hash (for rollback)
    error_msg        TEXT
);
```

### State machines

**Delivery level:**

```
PLANNING → READY → APPLYING → COMPLETED
```

- `PLANNING` → `READY`: Dependency resolution, CAS check, `transaction_ops`
  populated.  No system mutation yet — safe to discard on crash.
- `READY` → `APPLYING`: File copies, hook execution begin.  Commit marker.
- `APPLYING` → `COMPLETED`: Every op finished.  Delivery is done.

**Per-operation level:**

```
PENDING → EXTRACTING → HOOKS_RUNNING → DONE
```

Each status transition is written to the database *before* the corresponding
filesystem action, ensuring the WAL is always ahead of reality.

### Crash recovery

On startup, any `wright` command runs `delivery::recover_if_needed()`:

1. Find any delivery in `PLANNING`, `READY`, or `APPLYING`.
2. **PLANNING**: Mark `ROLLED_BACK`.  No mutation occurred.
3. **READY** or **APPLYING**:
   - Audit each operation:
     - `DONE` → already finished, skip.
     - `EXTRACTING` or `HOOKS_RUNNING` → mid-flight, reset to `PENDING`.
     - `PENDING` → never started, leave as-is.
     - `FAILED` → abort recovery, user must investigate.
   - Per-part filesystem cleanup is handled by `RollbackState::replay_journal`
     (see below) which runs on `TransactionContext::Drop`.
   - Mark delivery `ROLLED_BACK`, tell user to re-run the command.

On re-run, CAS provides pre-built parts for completed forge+seal work, so the
user only pays the cost of re-deploying.

---

## Per-Part Rollback Journal (unchanged)

`RollbackState` (`src/transaction/rollback.rs`) remains active as the
lowest-level safety net for individual file mutations within each
`TransactionContext`.  It tracks every file creation, backup, and symlink
change in a JSON-Lines journal, `fsync`-ed before each mutation.  On crash,
`replay_journal` undoes all changes in reverse order.

This is now a *nested* safety net inside the delivery-level WAL.  The WAL
tracks *which parts* need deployment; the rollback journal tracks *how* each
part's files can be undone.

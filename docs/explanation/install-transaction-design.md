# Install Transaction Design

Install, upgrade, and remove operations mutate the live filesystem. Wright
protects these operations with a rollback journal so that a crash or failure
never leaves the system in a half-installed state.

## Rollback Journal

`RollbackState` (`src/transaction/rollback.rs`) records every filesystem
mutation as a JSON Lines entry:

```json
{"FileCreated":{"path":"/usr/bin/foo"}}
{"DirCreated":{"path":"/usr/share/foo"}}
{"Backup":{"original":"/etc/foo.conf","backup":"/tmp/rollback/foo.conf.bak"}}
{"SymlinkBackup":{"original":"/usr/bin/bar","target":"/usr/bin/baz"}}
```

Each entry is `fsync`-ed to disk before the next mutation proceeds:

```rust
f.sync_data()?;  // ensure the journal entry is durable
```

This guarantees that the journal is always at least as complete as the
filesystem state. If the power fails, the journal contains every change that
may have been applied.

## Crash Recovery

When `RollbackState::with_journal` sees a leftover journal file, it replays the
entries in **reverse order** before starting any new transaction:

1. Remove newly created files.
2. Restore backed-up originals.
3. Restore symlink targets.
4. Remove newly created directories (children first).
5. Delete the journal.

After replay, the filesystem is back to the state it was in before the crashed
transaction began. The next run starts from a clean slate.

## Transaction Context

`TransactionContext` (`src/transaction/context.rs`) coordinates the rollback
state with the database audit log:

```rust
pub async fn commit(mut self) -> Result<()> {
    self.db.update_transaction_status(self.tx_id, TransactionStatus::Completed).await?;
    self.rollback.commit();  // deletes the journal file
    self.finalized = true;
    Ok(())
}

pub async fn rollback(mut self) -> Result<()> {
    self.rollback.rollback();
    self.db.update_transaction_status(self.tx_id, TransactionStatus::RolledBack).await?;
    self.finalized = true;
    Ok(())
}
```

The `Drop` implementation provides a last-resort safety net:

```rust
impl<'a> Drop for TransactionContext<'a> {
    fn drop(&mut self) {
        if !self.finalized {
            self.rollback.rollback();  // filesystem only; no async DB update
        }
    }
}
```

If a panic or early return bypasses both `commit()` and `rollback()`, the
filesystem is still restored. The database status is not updated in `Drop`
because async-in-Drop is unsafe; callers should explicitly call `rollback()` on
the error path for a complete audit trail.

## Commit Semantics

Success is signaled by **deleting the journal file**. If the journal still
exists, the transaction is considered unfinished and will be rolled back on the
next run. This is the same pattern used by SQLite and other journal-based
systems: the absence of the journal is the commit marker.

## Backup Strategy

Before overwriting an existing file, the install code creates a backup:

- Prefer `hard_link` (O(1), no extra disk space).
- Fall back to `copy` if hard-linking fails.

The backup path is recorded in the journal. On rollback, the original is
restored from the backup and the backup is removed.

For symlinks, the target path is recorded. On rollback, the symlink is
recreated with the original target.

## Atomic Placement

New files are placed using `rename(2)` when possible. When crossing filesystem
boundaries (EXDEV), the code falls back to copy-then-remove
(`src/transaction/fs.rs`). This is not atomic, but the journal ensures that a
crash during the copy leaves either the old file or the new file intact, never
a half-written file.

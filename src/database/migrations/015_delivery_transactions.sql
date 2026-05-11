-- V15: Delivery WAL — two-phase commit for system-mutation operations.
--
-- The installed registry records facts about packages on disk.  When wright
-- is _mutating_ the system (install/upgrade/remove), the mutation itself is
-- a multi-step transaction that must survive crashes.  These two tables form
-- the Write-Ahead Log (WAL) that governs delivery state machines.
--
--  delivery_transactions  —  one row per user-invoked command
--  transaction_ops        —  one row per DAG node action within the command

CREATE TABLE delivery_transactions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    command     TEXT    NOT NULL,                  -- e.g. "install nginx postgres"
    status      TEXT    NOT NULL DEFAULT 'planning', -- 'planning' | 'ready' | 'applying' | 'completed' | 'rolled_back'
    created_at  DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at  DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE transaction_ops (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id   INTEGER NOT NULL REFERENCES delivery_transactions(id),
    part_name        TEXT    NOT NULL,             -- e.g. "nginx"
    part_hash        TEXT    NOT NULL,             -- points to the concrete artifact in the CAS store
    action_type      TEXT    NOT NULL,             -- 'install' | 'upgrade' | 'remove'
    execution_order  INTEGER NOT NULL,             -- topological order from the DAG
    status           TEXT    NOT NULL DEFAULT 'pending', -- 'pending' | 'extracting' | 'hooks_running' | 'done' | 'failed'
    old_hash         TEXT,                         -- hash of the previous version (for rollback)
    error_msg        TEXT
);

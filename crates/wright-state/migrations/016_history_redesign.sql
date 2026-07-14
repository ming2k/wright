-- Replace old transactions table with a more robust history table
DROP TABLE IF EXISTS transactions;
DROP TABLE IF EXISTS history;

CREATE TABLE history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    session_id TEXT NOT NULL,
    command TEXT NOT NULL,
    part_name TEXT NOT NULL,
    action TEXT NOT NULL, -- 'install', 'upgrade', 'remove', 'rollback'
    old_version TEXT,
    new_version TEXT,
    old_hash TEXT,
    new_hash TEXT,
    status TEXT NOT NULL, -- 'completed', 'failed', 'rolled_back'
    details TEXT
);

CREATE INDEX idx_history_part_name ON history(part_name);
CREATE INDEX idx_history_session_id ON history(session_id);

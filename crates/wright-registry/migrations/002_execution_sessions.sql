CREATE TABLE IF NOT EXISTS execution_sessions (
    session_hash TEXT PRIMARY KEY,
    command_kind TEXT NOT NULL,
    task_session_hash TEXT,
    metadata_json TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS execution_session_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_hash TEXT NOT NULL,
    item_key TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(session_hash, item_key),
    FOREIGN KEY (session_hash) REFERENCES execution_sessions(session_hash) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_execution_sessions_command_kind
    ON execution_sessions(command_kind);
CREATE INDEX IF NOT EXISTS idx_execution_session_items_hash
    ON execution_session_items(session_hash);

INSERT OR IGNORE INTO execution_sessions (session_hash, command_kind, task_session_hash, created_at)
SELECT session_hash, 'build', session_hash, MIN(created_at)
FROM build_sessions
GROUP BY session_hash;

INSERT OR IGNORE INTO execution_session_items (session_hash, item_key, status, created_at)
SELECT session_hash, package_name, status, created_at
FROM build_sessions;

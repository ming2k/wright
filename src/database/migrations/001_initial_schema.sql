-- V1: Initial Clean Schema (Combined)

-- Core parts table with all metadata fields
CREATE TABLE parts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    version TEXT NOT NULL,
    release INTEGER NOT NULL,
    epoch INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    arch TEXT NOT NULL,
    license TEXT,
    url TEXT,
    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    install_size INTEGER,
    part_hash TEXT,
    install_scripts TEXT,
    assumed INTEGER NOT NULL DEFAULT 0,
    origin TEXT NOT NULL DEFAULT 'manual'
);

-- File tracking for each part
CREATE TABLE files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    file_hash TEXT,
    file_type TEXT NOT NULL,
    file_mode INTEGER,
    file_size INTEGER,
    is_config BOOLEAN DEFAULT 0,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

-- Part dependencies
CREATE TABLE dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    depends_on TEXT NOT NULL,
    version_constraint TEXT,
    dep_type TEXT DEFAULT 'runtime',
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

-- System transactions (install/upgrade/remove)
CREATE TABLE transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    operation TEXT NOT NULL,
    part_name TEXT NOT NULL,
    old_version TEXT,
    new_version TEXT,
    status TEXT NOT NULL,
    backup_path TEXT
);

-- Performance indices
CREATE INDEX idx_files_package ON files(part_id);
CREATE INDEX idx_files_path ON files(path);
CREATE INDEX idx_deps_package ON dependencies(part_id);
CREATE INDEX idx_deps_on ON dependencies(depends_on);

-- Shadowed files (for conflict analysis and safe removal)
CREATE TABLE shadowed_files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    original_owner_id INTEGER NOT NULL,
    shadowed_by_id INTEGER NOT NULL,
    diverted_to TEXT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (original_owner_id) REFERENCES parts(id) ON DELETE CASCADE,
    FOREIGN KEY (shadowed_by_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE INDEX idx_shadowed_path ON shadowed_files(path);

-- Optional (informational) dependencies, not enforced
CREATE TABLE optional_dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE INDEX idx_opt_deps_package ON optional_dependencies(part_id);

-- Virtual provides (e.g. http-server provided by nginx)
CREATE TABLE provides (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);
CREATE INDEX idx_provides_name ON provides(name);
CREATE INDEX idx_provides_package ON provides(part_id);

-- Part conflicts
CREATE TABLE conflicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);
CREATE INDEX idx_conflicts_name ON conflicts(name);
CREATE INDEX idx_conflicts_package ON conflicts(part_id);

-- Parts this package replaces (supersedes) at install time
CREATE TABLE replaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);
CREATE INDEX idx_replaces_name ON replaces(name);
CREATE INDEX idx_replaces_package ON replaces(part_id);

-- Build sessions: track progress of multi-package build runs for --resume
CREATE TABLE build_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_hash TEXT NOT NULL,
    package_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(session_hash, package_name)
);

CREATE INDEX idx_build_sessions_hash ON build_sessions(session_hash);

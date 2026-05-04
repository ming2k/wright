-- V5: Hard refactor — plan-centric registry with plan:output dependency syntax
--
-- This is a BREAKING migration that rebuilds the plans/parts schema.
-- Plan metadata is now registered at install time from pack archives.
-- Parts are outputs of a plan and must share the same version.

-- 1. Drop old tables and indexes (clean slate)
DROP TABLE IF EXISTS shadowed_files;
DROP TABLE IF EXISTS optional_dependencies;
DROP TABLE IF EXISTS files;
DROP TABLE IF EXISTS dependencies;
DROP TABLE IF EXISTS provides;
DROP TABLE IF EXISTS conflicts;
DROP TABLE IF EXISTS replaces;
DROP TABLE IF EXISTS transactions;
DROP TABLE IF EXISTS parts;
DROP TABLE IF EXISTS plans;
DROP INDEX IF EXISTS idx_files_package;
DROP INDEX IF EXISTS idx_files_path;
DROP INDEX IF EXISTS idx_deps_package;
DROP INDEX IF EXISTS idx_deps_on;
DROP INDEX IF EXISTS idx_shadowed_path;
DROP INDEX IF EXISTS idx_opt_deps_package;
DROP INDEX IF EXISTS idx_provides_name;
DROP INDEX IF EXISTS idx_provides_package;
DROP INDEX IF EXISTS idx_conflicts_name;
DROP INDEX IF EXISTS idx_conflicts_package;
DROP INDEX IF EXISTS idx_replaces_name;
DROP INDEX IF EXISTS idx_replaces_package;
DROP INDEX IF EXISTS idx_build_sessions_hash;
DROP INDEX IF EXISTS idx_parts_plan_name;
DROP INDEX IF EXISTS idx_parts_plan_id;
DROP TABLE IF EXISTS build_sessions;

-- 2. Create plans table — the registry of installed plans
--    Populated at install time from .PARTINFO in packs.
CREATE TABLE plans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,        -- plan name (e.g. "llvm")
    version TEXT NOT NULL,            -- plan version
    release INTEGER NOT NULL,         -- plan release
    epoch INTEGER NOT NULL DEFAULT 0, -- plan epoch
    description TEXT,
    arch TEXT NOT NULL,
    license TEXT,
    url TEXT,
    tools TEXT,                       -- JSON array of host tool names
    build_deps TEXT,                  -- JSON array of "plan:output"
    link_deps TEXT,                   -- JSON array of "plan:output"
    registered_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_plans_name ON plans(name);

-- 3. Create parts table — outputs of a plan
--    A plan can have multiple parts (outputs). All parts of a plan
--    MUST share the same version/release/epoch (enforced at install).
CREATE TABLE parts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,               -- output name (e.g. "clang")
    plan_id INTEGER NOT NULL,
    version TEXT NOT NULL,            -- mirrors plan.version
    release INTEGER NOT NULL,         -- mirrors plan.release
    epoch INTEGER NOT NULL DEFAULT 0, -- mirrors plan.epoch
    description TEXT,
    arch TEXT NOT NULL,
    license TEXT,
    url TEXT,
    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    install_size INTEGER,
    part_hash TEXT,
    install_scripts TEXT,
    assumed INTEGER NOT NULL DEFAULT 0,
    origin TEXT NOT NULL DEFAULT 'manual',

    FOREIGN KEY (plan_id) REFERENCES plans(id) ON DELETE CASCADE,
    UNIQUE(plan_id, name)
);

CREATE INDEX idx_parts_plan_id ON parts(plan_id);
CREATE INDEX idx_parts_name ON parts(name);

-- 4. File tracking for each part
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

CREATE INDEX idx_files_part ON files(part_id);
CREATE INDEX idx_files_path ON files(path);

-- 5. Part dependencies (runtime only; build/link are plan-level)
CREATE TABLE dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    depends_on TEXT NOT NULL,         -- "plan:output" format
    version_constraint TEXT,
    dep_type TEXT DEFAULT 'runtime',
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE INDEX idx_deps_part ON dependencies(part_id);
CREATE INDEX idx_deps_on ON dependencies(depends_on);

-- 6. Virtual provides
CREATE TABLE provides (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE INDEX idx_provides_name ON provides(name);
CREATE INDEX idx_provides_part ON provides(part_id);

-- 7. Conflicts
CREATE TABLE conflicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE INDEX idx_conflicts_name ON conflicts(name);
CREATE INDEX idx_conflicts_part ON conflicts(part_id);

-- 8. Replaces
CREATE TABLE replaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE INDEX idx_replaces_name ON replaces(name);
CREATE INDEX idx_replaces_part ON replaces(part_id);

-- 9. Shadowed files (for diversions)
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

-- 10. System transactions
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

-- 11. Build sessions
CREATE TABLE build_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_hash TEXT NOT NULL,
    package_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(session_hash, package_name)
);

CREATE INDEX idx_build_sessions_hash ON build_sessions(session_hash);

-- Reserve plan id 0 for assumed parts (no real plan association)
INSERT INTO plans (id, name, version, release, epoch, description, arch, license)
VALUES (0, '__assumed__', '0', 0, 0, 'placeholder for assumed parts', 'any', 'unknown');

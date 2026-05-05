-- V8: Fix UNIQUE constraint on parts; add CHECK on origin; drop dep_type from dependencies
--
-- UNIQUE(plan_id, name) allowed two plans to produce a part with the same name, which is
-- invalid — part names are globally unique identifiers. Replaced with UNIQUE(name).
--
-- dep_type is always 'runtime'; build/link deps live in separate tables. The column
-- and the Rust DepType enum are removed to eliminate a zero-value abstraction.

-- 1. Rebuild parts with UNIQUE(name) and CHECK(origin)
CREATE TABLE parts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    plan_id INTEGER NOT NULL,
    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    part_hash TEXT,
    install_scripts TEXT,
    origin TEXT NOT NULL DEFAULT 'manual'
        CHECK(origin IN ('dependency', 'build', 'manual', 'external')),
    FOREIGN KEY (plan_id) REFERENCES plans(id) ON DELETE CASCADE,
    UNIQUE(name)
);

INSERT INTO parts_new (id, name, plan_id, installed_at, part_hash, install_scripts, origin)
SELECT id, name, plan_id, installed_at, part_hash, install_scripts, origin
FROM parts;

DROP TABLE parts;
ALTER TABLE parts_new RENAME TO parts;

CREATE INDEX idx_parts_plan_id ON parts(plan_id);
CREATE INDEX idx_parts_name ON parts(name);

-- 2. Rebuild dependencies without dep_type
CREATE TABLE dependencies_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    depends_on TEXT NOT NULL,
    version_constraint TEXT,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

INSERT INTO dependencies_new (id, part_id, depends_on, version_constraint)
SELECT id, part_id, depends_on, version_constraint
FROM dependencies;

DROP TABLE dependencies;
ALTER TABLE dependencies_new RENAME TO dependencies;

CREATE INDEX idx_deps_package ON dependencies(part_id);
CREATE INDEX idx_deps_on ON dependencies(depends_on);

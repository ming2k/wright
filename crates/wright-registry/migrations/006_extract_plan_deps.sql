-- V6: Extract plan dependencies into separate tables, remove JSON fields
--
-- - Creates plan_build_deps and plan_link_deps tables (replacing JSON columns)
-- - Removes install_size from parts (computed on demand)
-- - Removes __assumed__ placeholder plan (assumed parts now register real plans)

-- 1. Create plan-level dependency tables
CREATE TABLE plan_build_deps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id INTEGER NOT NULL,
    depends_on TEXT NOT NULL,
    FOREIGN KEY (plan_id) REFERENCES plans(id) ON DELETE CASCADE,
    UNIQUE(plan_id, depends_on)
);

CREATE INDEX idx_plan_build_deps_plan ON plan_build_deps(plan_id);
CREATE INDEX idx_plan_build_deps_dep ON plan_build_deps(depends_on);

CREATE TABLE plan_link_deps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_id INTEGER NOT NULL,
    depends_on TEXT NOT NULL,
    FOREIGN KEY (plan_id) REFERENCES plans(id) ON DELETE CASCADE,
    UNIQUE(plan_id, depends_on)
);

CREATE INDEX idx_plan_link_deps_plan ON plan_link_deps(plan_id);
CREATE INDEX idx_plan_link_deps_dep ON plan_link_deps(depends_on);

-- 2. Migrate existing JSON data from plans table
-- Note: This handles the JSON array format used in V5
INSERT INTO plan_build_deps (plan_id, depends_on)
SELECT 
    p.id,
    json_each.value
FROM plans p,
     json_each(p.build_deps)
WHERE p.build_deps IS NOT NULL AND p.build_deps != '';

INSERT INTO plan_link_deps (plan_id, depends_on)
SELECT 
    p.id,
    json_each.value
FROM plans p,
     json_each(p.link_deps)
WHERE p.link_deps IS NOT NULL AND p.link_deps != '';

-- 3. Recreate plans table without JSON columns
CREATE TABLE plans_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    version TEXT NOT NULL,
    release INTEGER NOT NULL,
    epoch INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    arch TEXT NOT NULL,
    license TEXT,
    url TEXT,
    registered_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO plans_new (id, name, version, release, epoch, description, arch, license, url, registered_at)
SELECT id, name, version, release, epoch, description, arch, license, url, registered_at
FROM plans
WHERE name != '__assumed__';

DROP TABLE plans;
ALTER TABLE plans_new RENAME TO plans;

CREATE INDEX idx_plans_name ON plans(name);

-- 4. Recreate parts table with only part-level fields
CREATE TABLE parts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    plan_id INTEGER NOT NULL,
    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    part_hash TEXT,
    deploy_scripts TEXT,
    assumed INTEGER NOT NULL DEFAULT 0,
    origin TEXT NOT NULL DEFAULT 'manual',
    FOREIGN KEY (plan_id) REFERENCES plans(id) ON DELETE CASCADE,
    UNIQUE(plan_id, name)
);

INSERT INTO parts_new (id, name, plan_id, installed_at, part_hash, deploy_scripts, assumed, origin)
SELECT id, name, plan_id, installed_at, part_hash, deploy_scripts, assumed, origin
FROM parts;

DROP TABLE parts;
ALTER TABLE parts_new RENAME TO parts;

CREATE INDEX idx_parts_plan_id ON parts(plan_id);
CREATE INDEX idx_parts_name ON parts(name);

-- 5. Update foreign key references for assumed parts
-- Previously assumed parts had plan_id=0 (pointing to __assumed__).
-- Now they need real plans. We'll leave this to the application layer
-- to handle during the next assume/unassume operation.
-- Any remaining plan_id=0 references will be handled by the app.

-- 6. Recreate sessions table to use new parts table
-- (sessions don't reference parts directly, so no action needed)

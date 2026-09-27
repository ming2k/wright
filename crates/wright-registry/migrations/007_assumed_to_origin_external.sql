-- V7: Replace assumed boolean with Origin::External
--
-- The separate 'assumed' column is consolidated into the existing 'origin' field
-- as a new 'external' variant, removing the dual-field pattern where a part had
-- both an 'origin' and a distinct 'assumed' flag.

-- 1. Promote existing assumed rows to origin = 'external'
UPDATE parts SET origin = 'external' WHERE assumed = 1;

-- 2. Drop orphaned external parts whose plan reference was lost during V6
--    (plan_id = 0 pointed to __assumed__ which was removed in V6)
DELETE FROM parts
WHERE origin = 'external'
  AND plan_id NOT IN (SELECT id FROM plans);

-- 3. Rebuild parts table without the assumed column
CREATE TABLE parts_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    plan_id INTEGER NOT NULL,
    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    part_hash TEXT,
    deploy_scripts TEXT,
    origin TEXT NOT NULL DEFAULT 'manual',
    FOREIGN KEY (plan_id) REFERENCES plans(id) ON DELETE CASCADE,
    UNIQUE(plan_id, name)
);

INSERT INTO parts_new (id, name, plan_id, installed_at, part_hash, deploy_scripts, origin)
SELECT id, name, plan_id, installed_at, part_hash, deploy_scripts, origin
FROM parts;

DROP TABLE parts;
ALTER TABLE parts_new RENAME TO parts;

CREATE INDEX idx_parts_plan_id ON parts(plan_id);
CREATE INDEX idx_parts_name ON parts(name);

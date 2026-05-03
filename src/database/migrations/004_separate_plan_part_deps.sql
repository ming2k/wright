-- V4: Separate plan-level and part-level dependencies
-- 
-- Plan-level: build, link (stored in plans table)
-- Part-level: runtime (stored in dependencies table)

-- 1. Create plans table to store plan-level metadata
CREATE TABLE IF NOT EXISTS plans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    build_deps TEXT, -- JSON array of build dependency names
    link_deps TEXT,  -- JSON array of link dependency names
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- 2. Add plan_id column to parts for explicit plan relationship
ALTER TABLE parts ADD COLUMN plan_id INTEGER;

-- 3. Create index for plan lookups
CREATE INDEX IF NOT EXISTS idx_parts_plan_id ON parts(plan_id);

-- 4. Add foreign key constraint (SQLite supports this via table recreation)
-- Note: We keep plan_name as a denormalized string for backward compatibility
-- and use plan_id for referential integrity.

-- 5. Populate plan_id for existing parts
-- This will be done at runtime when plans are registered during apply/build

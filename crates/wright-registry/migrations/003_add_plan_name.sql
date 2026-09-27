-- V3: Add plan_name to parts table for plan-level lifecycle management

ALTER TABLE parts ADD COLUMN plan_name TEXT;

CREATE INDEX IF NOT EXISTS idx_parts_plan_name ON parts(plan_name);

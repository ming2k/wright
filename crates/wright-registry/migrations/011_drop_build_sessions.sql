-- Drop the legacy `build_sessions` table (V1).
--
-- The table was recreated in migration 005 (after a clean-slate drop)
-- but no Rust code has ever written to or read from it in the V4
-- workflow/step/run model.  It is pure dead weight.
--
-- Per CLAUDE.md, migrations are immutable, so we add a new migration
-- rather than editing 005 to stop recreating it.
DROP TABLE IF EXISTS build_sessions;

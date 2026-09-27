-- The execution_sessions / execution_session_items tables (added in 002)
-- backed the v3-era resume mechanism for `wright build` and `wright apply`.
-- The new workflow / step / run model (migration 009) supersedes them
-- entirely: workflow ids are content-addressed, runs are first-class, and
-- step state is the source of truth for resume.
--
-- Drop the legacy tables. Per CLAUDE.md, migrations are immutable, so we
-- supersede 002 with this drop migration rather than editing 002 in place.

DROP TABLE IF EXISTS execution_session_items;
DROP TABLE IF EXISTS execution_sessions;

-- V20: Plan-source snapshots move out of the database (ADR-0041).
--
-- Snapshots now live as plain files in the ledger
-- (`<ledger_dir>/<plan>/snapshots/<timestamp>-<checksum>.toml`), where
-- users can read and diff them with native tools — the table made that a
-- SQL query. Existing rows are exported to files by `InstalledDb::open`
-- *before* this migration runs, so the drop loses nothing. The
-- `plans.plan_checksum` column stays: it remains the relational pointer
-- drift detection joins on.

DROP TABLE IF EXISTS plan_snapshots;

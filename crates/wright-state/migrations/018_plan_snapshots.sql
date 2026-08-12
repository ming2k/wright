-- V18: Plan-source snapshots (ADR-0033).
--
-- Provenance checksums (V17) can say *that* plan source drifted; they cannot
-- say *what it was*. This table keeps the raw plan.toml text captured at seal
-- time, keyed by its SHA-256 — the same value `plans.plan_checksum` records.
-- Keying by checksum deduplicates re-seals of unchanged plans and retains
-- every distinct version ever deployed, so drift diffs and source recovery
-- work even after the plan file is edited or deleted.
--
-- Same stance as V17: descriptive audit data, never enforced. Rows are
-- inserted at part registration when the archive carries a `.PLANSRC`
-- member; deploy/resolve/remove never consult this table. Rows whose
-- checksum no plan row references anymore are retained deliberately — the
-- ledger keeps history.

CREATE TABLE plan_snapshots (
    checksum TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    recorded_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

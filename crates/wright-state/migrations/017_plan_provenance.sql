-- V17: Record seal-time provenance on plans (ADR-0023).
--
-- A part is a maintenance-ledger entry, so the ledger must be able to answer
-- "what produced this part". These columns mirror the `[provenance]` section
-- of `.PARTINFO` at registration time. They are descriptive facts, never
-- enforced (same stance as ADR-0016): `wright doctor` compares
-- `plan_checksum` against current plan source to report drift; nothing
-- blocks on a mismatch. Parts sealed before V17 leave all four NULL.
--
-- `source_checksums` is a JSON array of human-readable
-- "<kind> <locator> <verification>" strings copied verbatim from
-- `.PARTINFO`. It is diagnostic text, not a relation, so it is not
-- normalized into a table.

ALTER TABLE plans ADD COLUMN plan_checksum TEXT;
ALTER TABLE plans ADD COLUMN source_checksums TEXT;
ALTER TABLE plans ADD COLUMN wright_version TEXT;
ALTER TABLE plans ADD COLUMN isolation TEXT;

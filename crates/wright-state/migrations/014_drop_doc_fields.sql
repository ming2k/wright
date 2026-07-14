-- V14: Remove documentation fields from installed registry.
--
-- The installed registry records facts needed at runtime.  Description,
-- license, and URL are human-readable documentation that belongs in plan
-- source only.  If a user wants to know what a part does, they look at the
-- plan file; the database and `.PARTINFO` should not duplicate it.
--
-- Fields removed from `plans`:
--   * `description` — pure documentation, not used by any runtime logic.
--   * `license`     — legal metadata, irrelevant once the binary is on disk.
--   * `url`         — upstream project URL, already in plan source.
--
-- `arch` is retained because it is a runtime discriminator (you cannot mix
-- x86_64 and aarch64 binaries on the same root).

ALTER TABLE plans DROP COLUMN description;
ALTER TABLE plans DROP COLUMN license;
ALTER TABLE plans DROP COLUMN url;

-- V13: Adopt the advisory runtime-dep model.
--
-- Wright's installed registry now records *facts* about parts and the
-- relationships they declare. It does not enforce that runtime deps are
-- present — `installed`, `satisfied`, and `runnable` are three distinct
-- states. Strict completeness was a package-manager invariant that does
-- not fit a plan-centric, build-from-source orchestrator.
--
-- Concretely this migration drops three tables that no longer match the
-- model:
--
--   * `plan_build_deps`  — build deps describe the toolchain for a build,
--                          which is irrelevant once the binary is laid
--                          down. They live in plan source only.
--   * `plan_link_deps`   — link relationships are an empirical property
--                          of the produced binary (DT_NEEDED), not a
--                          declaration to persist. The plan-source field
--                          remains for the build pipeline; it just isn't
--                          mirrored into the installed db.
--   * `provides`         — virtual-name aliasing is a Debian-style
--                          abstraction that doesn't fit a plan-centric
--                          system, where every plan output is a first-
--                          class identifier. Part renames go through
--                          `replaces`; alternatives are a build-time
--                          variant concern, not a runtime one.
--
-- `dependencies` keeps `(part_id, depends_on, version_constraint)` —
-- still a soft TEXT pointer, never an FK. `version_constraint` survives
-- as diagnostic metadata (advisory, not enforced).

DROP INDEX IF EXISTS idx_plan_build_deps_plan;
DROP INDEX IF EXISTS idx_plan_build_deps_dep;
DROP TABLE IF EXISTS plan_build_deps;

DROP INDEX IF EXISTS idx_plan_link_deps_plan;
DROP INDEX IF EXISTS idx_plan_link_deps_dep;
DROP TABLE IF EXISTS plan_link_deps;

DROP INDEX IF EXISTS idx_provides_name;
DROP INDEX IF EXISTS idx_provides_part;
DROP INDEX IF EXISTS idx_provides_package;
DROP TABLE IF EXISTS provides;

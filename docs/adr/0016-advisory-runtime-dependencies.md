# ADR-0016: Advisory Runtime Dependencies

## Status

Accepted

## Context

Wright's installed registry historically tried to behave like a strict
package manager: when a part declared a runtime dependency, the resolver was
expected to ensure the dependency was registered before the depender, and
removal was guarded against breaking dependents. The dependency relations
shipped with the registry were:

- `dependencies (part_id, depends_on TEXT)` — runtime deps as soft TEXT
  pointers, but treated as if they should always resolve.
- `plan_build_deps (plan_id, depends_on)` — build-time toolchain.
- `plan_link_deps (plan_id, depends_on)` — declared link-time relationships.
- `provides (part_id, name)` — virtual aliases (e.g. `http-server`,
  `mta`) used for alternative resolution.
- `replaces`, `conflicts` — supersession and exclusion.

Strict completeness produced friction in every workflow that diverges from
"single rolling system kept fully consistent":

1. **Sysroot prep** must be allowed to install a deliberate subset.
2. **Cross-target staging** is half-broken until shipped.
3. **Mid-upgrade transients** briefly leave the registry unsatisfied.
4. **Plan source typos** generate permanently unresolvable names. With
   strict semantics, a single typo cascades into every operation that
   touches the part.
5. **Renames** require two-phase migrations because the old name disappears
   before the new alias is recognized.

Foreign keys on `dependencies.depends_on` were considered as a safety net
but rejected: they make case 1–4 impossible (the FK demands the target
exist) without addressing case 5 (FK refuses the rename it should permit).

`provides` accumulated complexity (virtual resolution in install, in
remove, in `wright check`) without earning its keep — Wright is plan-
centric, and every plan output is a first-class identifier. Alternatives
(musl vs. glibc, postfix vs. sendmail) are build-time variant choices, not
runtime resolution choices. The existence of `replaces` already covers
renames more honestly because it names a concrete prior identity rather
than fabricating a virtual one.

`plan_build_deps` and `plan_link_deps` recorded information that the
installed registry has no use for. Build deps describe a toolchain that is
irrelevant once the binary is laid down. Link deps describe declared
relationships that are at best redundant with the produced binary's
`DT_NEEDED` entries and at worst out of sync with reality.

## Decision

Wright adopts an **advisory** model for runtime dependencies. The
installed registry records facts about parts and their declarations; it
does not enforce that runtime targets exist.

### Three-state model

A part can be in any combination of these states, and they are not
collapsed into a single "installed" notion:

- **registered** — the row exists in `parts` and the files are on disk.
- **satisfied** — every `dependencies.depends_on` resolves (directly or
  via `replaces`).
- **runnable** — the part actually executes successfully.

`registered` does not imply `satisfied`. `satisfied` does not strictly
imply `runnable`. Each is a distinct query.

### Schema changes

- Drop `plan_build_deps`. Build deps live only in plan source for the
  build pipeline.
- Drop `plan_link_deps`. Empirical link relationships come from scanning
  the produced binaries' `DT_NEEDED` entries; declared link deps are not
  persisted.
- Drop `provides`. Plan-centric design has no use for virtual aliases;
  rename migrations go through `replaces`.
- Keep `dependencies (part_id, depends_on, version_constraint)` with
  `depends_on` as a soft TEXT pointer (no FK). `version_constraint` is
  retained as advisory diagnostic metadata; it is never enforced.
- Keep `replaces (part_id, name)` for rename migrations. Resolution
  walks `parts` first, falls through to `replaces`.
- Keep `conflicts (part_id, name)` as a hard install-time constraint —
  mutual exclusion is not advisory.

### Defensive layers (in lieu of FK enforcement)

- **Build-time validation.** The package step rejects a part whose
  declared runtime names cannot be resolved against the build sysroot or
  the known plan registry. Typos surface before the part is shipped.
- **Empirical link extraction.** Runtime deps are derived primarily from
  binary `DT_NEEDED` entries; manual declarations remain for dlopen and
  data-file dependencies.
- **`replaces` for renames.** `wright lint` enforces that a release whose
  output names diverge from the prior release includes the matching
  `replaces` entries.
 - **Diagnostic surface.** `wright check` and `wright launch` pre-flight
  together turn unsatisfied state into actionable user-facing information.

### Operational shape

 - `wright check` — read-only scan of `dependencies` against current
   registry; lists unsatisfied references.
- `wright remove` — warns when a removal will leave dependents unsatisfied
  but does not block; the user accepts or rejects the broken state.
- `wright launch` — pre-flight check; refuses to exec on unsatisfied
  registry state with an actionable hint.

## Alternatives considered

**FK on `dependencies.depends_on`.** Rejected. Strict existence is the
wrong invariant: it breaks sysroot prep, mid-upgrade transients, and
typos-as-diagnostics. It also fights `replaces` (the rename target FK
would refuse exactly the rename `replaces` exists to permit). FK gives
removal protection of the wrong shape (binary refuse-or-cascade) when
what is wanted is "warn, allow override".

**Resolved `part_id` snapshot column.** Considered: store the resolved
target id at install time so removal is FK-protected even with TEXT
sources. Rejected. Forces a "must be installed first" ordering that
breaks the same workflows. Adds fixup churn when targets are installed
later or renamed.

**Hash-based identity (Nix-style).** Rejected. Eliminates rename drift
and typo risk but loses readability and adds heavy machinery. Wright's
plan-centric, human-readable identity model would be undermined.

**Keep `provides` as inert metadata.** Considered to minimize churn.
Rejected because retaining a fake concept in plan source perpetuates the
confusion the migration is supposed to clear up. `provides` is removed
from the registry; the plan-source field will be removed in a follow-up.

## Consequences

### Positive

- Sysroot prep, partial systems, cross-target staging, and mid-upgrade
  transients are first-class supported, not edge cases.
- Typos and renames degrade gracefully into diagnostics rather than
  cascading failures.
- Resolver code simplifies dramatically: one fewer relation table, no
  virtual resolution, no version enforcement. `check_dependency` and
  `get_orphan_dependencies` lose their provides fallback joins.
- The mental model becomes honest: a registry that records facts, not a
  database that enforces correctness.

### Negative

- The "installed = working" assumption is gone. Users may install parts
  that do not run; this surfaces only at `wright launch` or via
  `wright check`. Mitigated by making the diagnostic surface excellent.
- Removal of a depended-on part no longer blocks at the database; the
  warning lives in application code. A `--force`-style escape is no
  longer needed because there is no hard wall, but the diagnostic must
  be loud enough that the user notices.
- `provides` is gone, so plans that previously depended on a virtual
  name must depend on a concrete `plan:output` or accept the constraint
  via `replaces`. This is a breaking change for any plan source using
  `provides`-style virtuals.

## Migration

- Migration `013_advisory_runtime_deps.sql` drops `plan_build_deps`,
  `plan_link_deps`, and `provides` tables.
- Database accessors for those tables are removed.
- `transaction/install.rs`, `transaction/upgrade.rs`,
  `transaction/remove.rs`, and `query/mod.rs` are updated to drop
  `find_providers` / `insert_provides` / `get_provides` calls.
- `manifest.relations.provides` remains in plan source as an inert field;
  a follow-up will remove it after a deprecation window.
- `.PARTINFO` no longer contains `provides`, `build_deps`, `link_deps`,
  `description`, or `license`.  The binary part metadata carries only
  install-time/runtime facts.  Human-readable documentation belongs in plan
  source only.
- Build/link deps remain in plan source and continue to drive the build
  pipeline (`forge/mvp.rs`, `planning/graph.rs`); only the registry
  persistence is removed.

## Related

- [ADR-0009: Separate plan-level and output-level dependencies](0009-separate-plan-output-dependencies.md)
  — established that runtime deps are output-level. This ADR refines
  what the registry does with them.
- `docs/explanation/dependency-philosophy.md` — user-facing exposition of
  the model.

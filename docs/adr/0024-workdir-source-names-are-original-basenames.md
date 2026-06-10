# ADR-0024: Work-Directory Source Names Are Original Basenames

## Status

Accepted

## Context

Every cached source file is stored in the shared source cache
(`source_dir`) under the name `<part_name>-<basename>`. The prefix exists
for exactly one reason: the cache is shared across all plans, and two plans
may legitimately ship files with the same basename (`config`, `LICENSE`,
`default.conf`).

Until 5.3.10, the `extract` step of the charge pipeline copied non-archive
http and local sources into the per-plan work directory under that same
cache name. The prefix — a cache-namespacing detail — leaked into
`${WORKDIR}`, with three consequences:

- Plans had to reference awkward doubled names. A plan named `sing-box`
  shipping `sing-box.service` had to write
  `${WORKDIR}/sing-box-sing-box.service`.
- The reference documentation ("non-archive files are copied directly to
  `${WORKDIR}`") described the intended behavior, not the actual one.
  Plan authors discovered the prefix only when their staging script failed.
- Two sources with the same basename silently overwrote each other, both in
  the cache and in the work directory.

`http` sources already had an `as` field overriding the cached filename;
`local` sources had no equivalent escape hatch.

## Decision

### 1. The prefix stays in the cache, and only in the cache

`source_cache_filename` (`<part_name>-<basename>`) remains the storage name
in the shared source cache. Nothing about cache layout, reuse, or
verification changes.

### 2. Work-directory names are the source's own basename

When `extract` copies a non-archive http or local source into `${WORKDIR}`
(or its `extract_to` subdirectory), the destination filename is the
source's own basename: the last path segment of `url` or `path`. A plan
references the file by the same name it has in the plan directory or in the
URL.

### 3. `as` overrides both names, on both source types

`local` sources gain the `as` field that `http` sources already had. When
set, `as` names the file in both the source cache and the work directory.
It is the escape hatch for basename collisions and for giving a meaningful
name to an opaque download.

### 4. Destination collisions are errors

Two sources of one plan that resolve to the same work-directory file abort
the charge stage with an error naming the colliding path, instead of the
last copy silently winning. The error suggests renaming one source with
`as`.

## Alternatives considered

- **Keep the prefixed names and fix only the documentation.** Rejected:
  the doubled-name case (`sing-box-sing-box.service`) shows the convention
  is hostile to plan authors, and the prefix serves no purpose inside a
  per-plan directory.
- **Strip the prefix in the cache as well.** Rejected: the cache is shared
  across plans, so basenames alone collide there. The prefix is correct at
  that layer.
- **Detect collisions at fetch time instead of extract time.** Not pursued:
  `prepare` always runs fetch → verify → extract as one unit, so an
  extract-time check surfaces before any build script runs. Fetch-time
  detection would catch cache-level collisions slightly earlier at the cost
  of duplicating the destination-name computation.

## Consequences

### Positive

- Plan scripts reference sources by their natural names; the plan directory
  listing and the work directory agree.
- The reference documentation is now literally true.
- Same-basename collisions fail loudly with an actionable message.
- Plans that referenced URL basenames directly (and were silently broken
  under the old naming) now work as written.

### Negative

- **Breaking change.** Existing plans referencing
  `${WORKDIR}/<part_name>-<file>` must drop the prefix. All plans in the
  system plan tree were migrated in lockstep with the 5.3.10 release.
- `as` on a local source changes its cache name, so switching `as` on an
  already-cached source re-copies it under the new name, leaving the old
  cache entry behind until cleaned.

## Related

- ADR-0004: No magic behavior
- [Plan Manifest Reference](../reference/plan-manifest.md) — `[[sources]]`
  field tables

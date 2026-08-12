# ADR-0034: Plan/Output Namespaces and Plan-Qualified Part Layout

## Status

Accepted

## Context

A plan builds one or more outputs; a deployed output is a part. The two
names were historically treated as one namespace: the artifact filename,
the `parts.name` uniqueness constraint, dependency edges, and store
lookups all keyed on the bare output name, while plan names lived in a
parallel table with no rules relating the two. Nothing stopped plan `a`
from declaring an output named `x` while a plan `x` also existed, and the
system misbehaved under that shadowing in ways an audit made concrete:

- **Artifacts collided on disk.** The flat
  `<output>-<ver>-<rel>-<arch>.wright.tar.zst` layout in `parts_dir` let
  one plan's archive silently overwrite another plan's same-named output
  on an identical version tuple, and store/reuse/prune could not tell
  same-named archives apart.
- **Resolution conflated the namespaces.** A bare dependency name was
  satisfied by any same-named part regardless of its plan, reverse-
  dependency expansion matched plan names against part names, and the
  build graph's output→plan map was nondeterministic under contention.
- **Deploy could corrupt provenance.** Redeploying a same-named part from
  a different plan silently re-parented the part row, and removing a part
  could delete an unrelated same-named plan's registry row.
- **Qualified dependency edges were invisible.** `depends_on` stored
  `plan:output` verbatim while dependent/orphan queries matched literally,
  so a `plan:output` dep neither blocked nor cascaded at removal time.

Forbidding every cross-plan name overlap was considered too strict for a
tolerated, sometimes useful authoring freedom; the failures came instead
from the system pretending the namespaces were one.

## Decision

Plans and outputs are distinct namespaces, and every layer carries both
when identifying an artifact:

1. **Namespace policy.** Several plans may declare the same output name.
   Deployed part names remain globally unique, so same-named outputs of
   different plans can never co-exist on one system — the shared target
   filesystem (`/usr/bin/x` has one owner) makes global uniqueness a
   feature, not a limitation. A plan named exactly like another plan's
   output is legal; tooling disambiguates rather than forbids.
2. **Universal target identifier.** Every user-facing target resolves
   through one identifier grammar: `plan` or `plan:*` addresses all
   deployed outputs of a plan, `output` or `plan:output` addresses one
   output. A bare name matching both namespaces is rejected as ambiguous
   and the error names the absolute forms. `files`, `remove`, `check`,
   `history`, and `merge` all resolve through it.
3. **Artifact identity and layout.** Archive identity is the
   `(plan, output)` pair read from `.PARTINFO`, never the path. New
   archives seal to `parts_dir/<plan>/<output>-<ver>-<rel>-<arch>
   .wright.tar.zst`; all store scans recurse, so legacy flat archives
   remain readable. Resolution that knows the originating plan is
   plan-pinned; prune groups by `(plan, output)`; clean and doctor match
   archives by plan metadata.
4. **Dependency edges** store the bare output (part) name — the canonical
   key of the globally unique deployed namespace. `plan:output` refs are
   normalized at registration; migration V19 rewrites existing rows.
5. **Deploy guards.** Redeploying a part whose name is deployed under a
   different plan is a hard error unless the incoming part declares the
   old name in `replaces` (the rename path) or the user passes `--force`
   (a loud warning, never silent). `wright provide` refuses a name owned
   by a plan with deployed parts. Resolution compares a dependency's own
   plan record only. `wright lint` validates dependency references
   (missing plan warns; undeclared `plan:output` errors) and warns on
   cross-plan name collisions.

## Alternatives

- **Forbid overlapping names at authoring time.** A lint error whenever
  an output name matches another plan's name or another plan's output.
  Rejected: plan names and output names are genuinely different
  vocabularies (a plan `docs` and an output `docs` are both natural), the
  database already prevents harmful co-deployment, and a ban cannot fix
  the pre-existing installed base. Lint warns instead.
- **Composite part identity.** Drop global uniqueness and key parts by
  `(plan, output)` everywhere — schema, edges, dependents, hooks. Rejected
  as disproportionate: the deployed filesystem is a single namespace, so
  co-deployed same-named parts would collide on file ownership anyway;
  the global part name is the correct deployed key.
- **Plan prefix inside a flat filename** (`<plan>+<output>-...`). Viable,
  but subdirectory grouping mirrors distro pool layouts, keeps filenames
  short, and needs the same recursive scan either way.

## Consequences

- Existing `parts_dir` contents keep working: scans recurse and identity
  comes from `.PARTINFO`, so flat and nested archives coexist.
- Migration V19 rewrites qualified `depends_on` rows in place; edges
  become visible to removal blocking and orphan cascades.
- Commands that previously guessed a namespace now fail loudly on
  ambiguous bare names — a deliberate behavior change, documented in the
  CLI reference and changelog.
- Plan authors gain deterministic, lint-visible rules: shadowing is
  reported with the exact qualified spelling to use.
- Future name-bearing features must respect the two namespaces: carry the
  plan alongside the output wherever an artifact is identified, and route
  user targets through the shared identifier.

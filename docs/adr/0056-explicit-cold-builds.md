---
id: ADR-0056
title: "Explicit Cold Builds: `--no-cache` Purges Shared Caches and Denies Every Reuse Decision"
status: superseded
date: 2026-10-27
scope: cli, core/foundry
superseded_by: ADR-0057
negative_knowledge: true
---

# 0056. Explicit Cold Builds: `--no-cache` Purges Shared Caches and Denies Every Reuse Decision

- Status: Superseded by [ADR-0057](0057-transactional-cold-purge-and-trust-boundary.md)
- Date: 2026-10-27
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

ADR-0054 gave every source a content address and made that address the single
source-derived input to the charge marker, the build key, and the forge
checkpoint chain. It deliberately left one hole open: a git source pinned to a
**named ref** is addressed by `url + ref + snapshot filename`, not by the
commit the ref currently points at. The snapshot is an explicit pin (ADR-0038),
and ADR-0054's rejected-options section names the remedy — "refreshed by
clearing the source cache" — without providing a safe way to do it.

The result is that `wright upgrade` cannot see a moved `main`/`v1` ref, and
`--force` does not help: it bypasses the CAS and the forge checkpoints but still
reuses `source_dir`, so the old snapshot is re-extracted and the old bytes are
sealed and deployed. Operators are left with `rm -rf` of a global cache
directory, which is undeclared behavior, unguarded by the lock, and invisible
to `--dry-run`.

At the same time, ADR-0054 Rejected Alternative 5 forbids making correctness
depend on the operator remembering a flag. `--no-cache` is therefore not a fix
for a cache that can go stale: the caches are correct under their stated
contract. It is an **operator control** for the one case the contract
deliberately does not cover — "do not trust any locally cached byte for this
run".

The existing flags do not carry this responsibility:

- `--fresh` (-c) wipes only the per-plan forge workspace. The source cache, the
  CAS, and the resolve-level `Outdated` filter are untouched.
- `--force` (-f) bypasses the CAS, checkpoints, and deploy hash-skip, but keeps
  the source cache and, on its own, does not clear the workspace.
- Neither purges shared caches, and neither is a name an operator should read as
  "start over from upstream".

---

## Decision Drivers

- **One explicit name for one job.** "Rebuild from freshly fetched sources,
  trusting nothing cached" is a distinct intent; composing it out of `--force`
  plus manual deletion is exactly the kind of operator ritual that produces
  silent wrong answers.
- **Never delete the inventory.** `parts_dir` archives are the local inventory
  and the rollback copy (ADR-0043). A build mode must not destroy them to prove
  it did not reuse them.
- **No implicit behavior.** A cold mode that can be enabled by configuration or
  environment would let a global switch silently multiply build times
  (ADR-0004, ADR-0043's rejected automatic GC).
- **Preview before destruction.** The purge is global and affects plans other
  than the target; `--dry-run` must report the exact set before anything is
  removed (ADR-0040's safety model).
- **Existing meanings are frozen.** `--force` and `--fresh` keep their
  documented semantics (ADR-0054 Rejected Alternative 5).

---

## Decision Outcome

Chosen option: **a long-only `--no-cache` flag on `wright build`, `wright
install`, and `wright upgrade`**, with two effects:

1. **Purge the two pure caches**, before any cache decision:
   - `general.source_dir` (every entry).
   - `general.store_dir` (every CAS entry).
   Roots that do not exist are empty caches. An existing root that is a symlink
   or not a directory fails closed. Nothing else is deleted — not `parts_dir`,
   not the registry, not the ledger, not logs.

   `build` does not read the CAS itself, yet it purges it too. Leaving it would
   let a later ordinary `install` resolve the *pre-cold* artifact from the CAS:
   a named git ref keeps its `SourceDigest` when it moves, so the old entry still
   answers for the new source. A purge that is not complete across the two pure
   caches is therefore not a cold start at all.
2. **Deny every derived reuse for the whole resolved build set**:
   resolve-level `Outdated` filtering (`preserve_targets`), the charge marker,
   the source cache (`fetch` re-downloads even if an entry reappeared), forge
   checkpoints, the staging cache, `layers/` and `base/`, CAS lookup, seal's
   reuse of existing `outputs/`, deploy's archive-hash skip, and ABI rebuild
   inhibition.

`--no-cache` implies the workspace wipe that `--fresh` performs, but it does not
set `force`: the two flags remain separate dimensions. `upgrade all --no-cache`
selects **every installed plan**, because a cold rebuild is not a staleness
judgement; plans whose local `plan.toml` has disappeared are skipped with a
warning. `wright launch` and `wright merge` do not expose the mode in this
decision: `launch` fills a fresh root and already composes `--fresh`/`--force`,
and `merge` deploys existing archives rather than building from sources.

Cache writes are permitted. A cold build may repopulate `source_dir`, the CAS,
and the marker; those writes describe what this invocation just built and can
never answer for it, because the read paths are disabled for the run.

### Invariants & Behavioral Boundaries

- **`[INV-COLD-01]` Explicit only.** `--no-cache` has no configuration key, no
  environment default, and no implicit trigger.
- **`[INV-COLD-02]` Purge both pure caches, and only them.** Every command that
  exposes `--no-cache` purges `source_dir` and the CAS `store_dir`.
  `parts_dir`, the registry, the ledger, and logs MUST NOT be deleted by the
  purge (ADR-0043: archives are inventory, not cache).
- **`[INV-COLD-03]` No derived read survives.** For the selected build set, no
  marker, checkpoint, staging cache, layer, base manifest, CAS entry,
  `Outdated` filter, or deploy hash comparison may answer.
- **`[INV-COLD-04]` Previewable and fail-closed.** `--dry-run` reports the purge
  set (roots, entry counts, bytes) and the resolved execution plan, with no
  deletion and no network I/O. Each root is validated before any deletion; one
  invalid root aborts the whole purge.
- **`[INV-COLD-05]` Writes are not reads.** A cold build MAY repopulate any
  cache it bypassed; a cache write records what was just built and MUST NOT be
  consulted again in the same invocation.
- **`[INV-COLD-06]` Existing flags keep their meanings.** `--force` still reuses
  the source cache; `--fresh` still clears only the workspace. `--no-cache` is
  strictly stronger and redefines neither.

### Positive Consequences

- A moved named git ref is observable through the CLI: `wright install
  --no-cache <plan>` re-fetches, re-forges, re-seals, and deploys the new
  content, with no manual deletion of a global directory.
- The purge is declared, previewable, lock-protected, and limited to pure
  caches. A failed cold build loses only re-fetchable data; the previous archive
  remains deployable.
- The operator no longer composes `--force` with `rm -rf` to mean "start over",
  so the intent is recorded in the command and in the ledger.

### Negative Consequences & Trade-offs

- The purge is global, not per-plan: `wright build --no-cache <plan>` also
  discards the source cache and CAS entries of unrelated plans, which will
  rebuild or re-download on their next ordinary run. This is inherent — source
  cache filenames have no reliable plan dimension (`as` aliases may be shared).
- `upgrade all --no-cache` is a full-system cold rebuild. `--dry-run` is the
  guard, and the documentation says so.
- Pinned `http` sources are re-downloaded even though their declared `sha256`
  already proves their content. That cost is the point of the mode; operators
  who want "verify cached bytes" keep the default path.
- A purge must exclude ordinary builds, which hold only the `build` lock, so
  every purge-capable command takes the shared `wright` lock and then the
  `build` lock (fixed order; `build --no-cache` takes the same pair, ordinary
  `build` keeps only its own lock). Without that, a normal build could be
  extracting sources while a cold install deletes them.

---

## Rejected Alternatives & Negative Knowledge

### 1. Make `--force` refresh the source cache too

- **Why considered**: Zero new surface; `--force` is the existing "do it again"
  flag.
- **Why rejected**: ADR-0054 Rejected Alternative 5 fixes `--force`'s meaning
  on purpose: it is a rebuild control, not a cache-trust control. Overloading it
  would silently turn every forced rebuild into a global re-download, and would
  leave no way to force a reforge while keeping a warm source cache.

### 2. Add a source-only `--refresh-sources`

- **Why considered**: Smaller blast radius; only the network layer is refreshed.
- **Why rejected**: It does not work. A named ref's snapshot filename and
  `SourceDigest` are unchanged when the ref moves, so the charge marker reuses
  the previous `source/` tree and the CAS returns the previous artifact. Source
  freshness without upper-layer bypass is a no-op for the case that motivated
  the feature.

### 3. A `[build] no_cache = true` configuration key or `WRIGHT_NO_CACHE`

- **Why considered**: Convenient for CI images and air-gapped profiles.
- **Why rejected**: A global switch that multiplies every build's cost and
  silently disables caching violates ADR-0004 (no implicit behavior) and
  ADR-0043's rejection of install-side automatic GC. The mode is per-invocation
  by design; CI can put the flag in its command line.

### 4. A new `wright rebuild` subcommand

- **Why considered**: A verb is clearer than a flag, and `clean`/`prune` set a
  precedent for verb-level maintenance.
- **Why rejected**: It would duplicate `upgrade`'s target selection and
  `install`'s option surface (`--deps`, `--rdeps`, `--match`, `--root`,
  `--dry-run`, hooks). The capability composes with every existing install
  option; a flag composes for free.

### 5. Delete the target plans' archives from `parts_dir`

- **Why considered**: "No cache" reads as "leave nothing behind", and a stale
  archive is the most visible local artifact.
- **Why rejected**: `parts_dir` is the inventory, not a cache (ADR-0043): it is
  the source of truth `wright doctor --repair` rebuilds from, and the rollback
  copy if the new build fails. Nothing in the build path reuses an archive by
  existence anyway; seal overwrites it. Deleting it buys no correctness and
  removes recovery.

### 6. Purge only the caches belonging to the selected plans

- **Why considered**: A non-destructive, minimally surprising blast radius.
- **Why rejected**: The source cache has no plan dimension — an `as` alias can
  be shared, and git snapshot names ignore it — so a plan-scoped purge cannot be
  made complete. A partial purge would leave entries that the cold run must
  still refuse to read, which is exactly the undeclared behavior this ADR
  removes.

### 7. Keep the cache after the cold build

- **Why considered**: A cold build is a verification tool; leaving the refreshed
  artifacts in place makes the next ordinary build fast.
- **Why accepted in part, not rejected**: The implementation deliberately
  repopulates the caches with what the cold build produced
  (`[INV-COLD-05]`). The rejected variant is the stronger "delete everything
  afterwards", which would require a second teardown pass with its own failure
  modes and would discard genuinely fresh content.

---

## Links

- Related ADRs: [ADR-0004](0004-no-magic-behavior.md),
  [ADR-0038](0038-git-source-snapshots.md),
  [ADR-0040](0040-clean-prune-consolidation-and-fresh-flag.md),
  [ADR-0043](0043-registry-as-derived-index-and-maintenance-surface.md),
  [ADR-0054](0054-content-addressed-source-identity.md)
- Implementation:
  `crates/wright-actions/src/operations/cache_purge.rs`,
  `crates/wright-actions/src/foundry/charge/mod.rs`,
  `crates/wright-actions/src/foundry/charge/fetch.rs`,
  `crates/wright-actions/src/foundry/forge/mod.rs`,
  `crates/wright-actions/src/operations/install/execute.rs`,
  `crates/wright-actions/src/operations/upgrade.rs`,
  `crates/wright-actions/src/operations/build.rs`,
  `src/cli/{build,install,upgrade}.rs`
- Narrative: [Build Mechanics](../reference/build-mechanics.md),
  [Plan Build](../explanation/plan-build.md),
  [Install Design](../explanation/install-design.md)

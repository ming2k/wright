---
id: ADR-0054
title: "Content-Addressed Source Identity for Every Cache Key"
status: accepted
date: 2026-10-26
scope: core/foundry
superseded_by: null
negative_knowledge: true
---

# 0054. Content-Addressed Source Identity for Every Cache Key

- Status: Accepted
- Date: 2026-10-26
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

Three independent memoization layers sat between a `plan.toml` and the bytes it
deploys. Each was keyed on the plan's **declaration** of its sources rather than
on the **resolved content** of those sources:

1. **Charge marker** (`.charge_prepared`). `Charge::fingerprint` hashed a
   source's fields as strings. For `type = "local"` that meant
   `hasher.update(local.path.as_bytes())` — the *path* — and never the file
   body.
2. **Build key** (`Foundry::compute_build_key`, consumed by
   `PlanFingerprints` → `BuildCache::compute_closure_fingerprint` → the CAS).
   Same defect: the `Source::Local` arm hashed `local.path`, not bytes.
3. **Forge checkpoint chain** (`.wright-checkpoint.json`). Stage input hashes
   were chained from `prev_hash = ""` and covered only script, executor,
   isolation, and environment text. The materialized `source/` tree
   contributed nothing to the chain root.

The three layers fail independently and compose into a total loss of
correctness. Editing a local source — the canonical case being a systemd unit
shipped via `[[sources]] type = "local"` — changes none of the three keys:

- Charge sees an unchanged marker fingerprint and returns the previously
  extracted `source/` tree, never re-copying the edited file.
- The build key is unchanged, so the CAS resolves the previously sealed part and
  forge/seal are skipped entirely.
- Even when the CAS misses, every stage hash is unchanged, so
  `find_rewind_point` reports "all up-to-date" and `staging/` is restored from
  `.staging_cache/`.

`wright upgrade` then runs the entire `Building → Staging → Sealing →
Deploying → Finished` ceremony and redeploys the stale artifact. The command
reports success, `cmp` on the deployed file reports no change, and the operator
re-reads an identical error. The tool is not merely wrong; it is *confidently*
wrong, and it charges full ceremony to be so.

Two structural faults underlie the three symptoms. First, `.charge_prepared`
marked the *declared* input set, so it could only ever be a cache of a
declaration. Second, the forge hash chain had no root anchored in the
materialized source tree, so the chain could not express "the sources changed".

ADR-0033 and ADR-0038 establish that source artefacts must be reproducible and
inspectable, and ADR-0048 makes content addressing the storage primitive. A
cache key derived from a declaration contradicts all three.

---

## Decision Drivers

- **Correctness over speed.** A cache that cannot go stale is worth more than a
  cache that is sometimes wrong; a silent stale deploy is the worst outcome the
  system can produce.
- **Single derivation.** Three ad-hoc key derivations are three chances to miss
  a fourth. The number of places that must be right must be one.
- **No hint-based identity.** mtime, size, and path are hints about content, not
  content.
- **Offline and zero-second reuse must survive.** The fix must not force network
  access or full re-materialization on every build.
- **Abstention over guessing.** Where content cannot be known cheaply, the
  system must decline to reuse rather than assume equality.

---

## Decision Outcome

**A plan's fully resolved source set has a content address — `SourceDigest` —
and that digest is the sole source-derived input to every cache key.**

### 1. The digest

`Charge::resolve_sources(manifest, plan_dir)` produces a
`SourceDigest { hex, volatile }`. The hex is SHA-256 over a version-prefixed,
length-delimited record of every `[[sources]]` entry in declaration order. The
per-source *content address* is:

| Source | Content address |
|--------|-----------------|
| `local` | SHA-256 of the file bytes at `<plan_dir>/<path>` |
| `http` with `sha256 = "<hex>"` | the declared `sha256` |
| `http` with `sha256 = "SKIP"` | SHA-256 of the cached artefact, once fetched |
| `git` (snapshot) | the declared URL + ref (ADR-0038 pins one snapshot per ref) |
| `git_metadata = true` | URL + ref — **volatile** (always re-cloned, never cached) |

The record also carries each entry's `as` and `extract_to`, and a format
version string. The version prefix is the clean break: a marker written by the
previous algorithm can never match a v1 digest, so the first build after this
change re-materializes every plan once, then converges.

### 2. Volatility

A source is **volatile** when its content cannot be known without doing the
work: `git_metadata = true` (always fetched by design), and an `http` `SKIP`
source whose artefact is not yet in the source cache. `SourceDigest::volatile`
is the OR over the plan's sources.

Volatile sources are handled by *abstention*, not by guessing:

- Charge never short-circuits; it always materializes.
- The forge never honours a checkpoint (`can_checkpoint()` is false), so all
  stages re-run.
- The CAS is neither consulted nor populated for the plan.

A plan whose inputs are not content-addressable gets no reuse. That is the
honest answer, and it matches ADR-0038's own stance that `git_metadata` sources
always fetch from the network.

### 3. Propagation

1. **Charge** (`crates/wright-actions/src/foundry/charge/`). The marker stores
   the digest; the short-circuit requires a matching marker, a present
   `source/`, and `!volatile`. `ChargeResult` carries the digest forward.
2. **Build key** (`Foundry::compute_build_key`). Hashes plan metadata, the
   pipeline stages, and `SourceDigest::as_str()`. The per-source string loop is
   deleted.
3. **Forge** (`ForgeContext.source_digest`). The digest *seeds* the stage hash
   chain (`compute_expected_hashes(.., seed)`, `prev_hash = seed`), so any
   source change mismatches the first stage and rewinds the whole pipeline.
   `Checkpoint` persists the seed it was built from, so the cheap pre-build
   short-circuit in `wright build` compares it without re-deriving the chain.

The obsolete `ForgeContext.build_key` field — computed on every build, stored in
no struct, read by no code — is removed rather than repaired.

### 4. Convergence for unpinned sources

A `SKIP` source resolves to `unresolved` before its first fetch and to the
artefact hash afterwards. The marker is written from identities resolved *after*
materialization, so build 1 records the real hash and build 2 onward
short-circuit. No unpinned source is memoized while unresolved.

---

## Invariants

- **`[INV-SRC-01]` Single Content-Addressed Source Identity**: Every cache key
  derived from a plan's sources MUST be derived from `SourceDigest`. No cache
  key in charge, the forge checkpoint chain, or the CAS may hash a source's
  declaration (path, URL, or checksum string) directly. There is exactly one
  implementation of the digest.
- **`[INV-SRC-02]` Local Sources Are Keyed By Bytes**: A `type = "local"`
  source's content address MUST be the SHA-256 of its file bytes. Path, size,
  and mtime MUST NOT participate. A build that resolves an unchanged digest for
  a changed local file is a correctness defect, not a tuning opportunity.
- **`[INV-SRC-03]` The Stage Chain Has A Root**: The forge stage hash chain MUST
  be seeded with the plan's `SourceDigest`. A chain rooted in a constant cannot
  express a source change and MUST NOT be trusted.
- **`[INV-SRC-04]` Volatility Abstains, Never Guesses**: A plan containing a
  non-content-addressable source MUST be denied all reuse — charge
  short-circuit, stage checkpoints, and CAS read and write. Falling back to a
  declaration-derived key for such a plan is prohibited.
- **`[INV-SRC-05]` Reuse Requires Proven Inputs**: A cached artefact (marker,
  checkpoint record, or CAS entry) may answer for a build only after its
  recorded source identity has been compared with the current `SourceDigest`.
  A cache backend that cannot be interrogated on this point MUST be bypassed.
- **`[INV-SRC-06]` Fail Closed On Unreadable Local Sources**: A `local` source
  that is missing, unreadable, escapes the plan directory, or is not a regular
  file MUST abort the build with a diagnostic naming the path. It MUST NOT be
  silently treated as absent, empty, or unchanged.

---

## Rejected Alternatives & Negative Knowledge

### 1. Compare mtimes instead of content
- **Why considered**: Cheaper than hashing; conventional for build tools.
- **Why rejected**: An mtime is a hint about content, not content. `cp`,
  `rsync -a`, `tar -x`, a checkout, or a restored backup all reset it, and a
  same-second edit is invisible. It also cannot distinguish "restored to the
  previous bytes" (cache valid) from "touched" (cache needlessly discarded).
  ADR-0038 already rejects hint-based identity by zeroing snapshot entry mtimes.

### 2. Hash the materialized `source/` tree as the only identity
- **Why considered**: Perfectly content-true, and it subsumes every source
  type — including `git_metadata` clones and moving refs.
- **Why rejected**: It inverts the decision it is meant to serve. You must
  extract before you can decide whether extraction was necessary, so the charge
  short-circuit — the entire point of the marker — becomes impossible, and every
  build pays a full tree hash of a source tree that may be hundreds of
  megabytes. The primitive is not discarded: `compute_dir_manifest` already
  provides it for staging-output integrity, which is where the
  materialized-artefact guarantee belongs.

### 3. Record the resolved git commit in a sidecar and key on it
- **Why considered**: Makes git sources content-addressed even for named refs.
- **Why rejected**: A named ref's cached snapshot is an *explicit pin* under
  ADR-0038 — one snapshot per ref, refreshed by clearing the source cache.
  Recording the commit makes the digest depend on which caches happen to be
  warm, so `wright clean --sources` would silently invalidate every downstream
  key and rebuild the world even when the commit did not move, and two machines
  with the same plan would compute different build keys. Commit-hash refs are
  already content-addressed by their own `ref`.

### 4. Fix the three cache layers independently
- **Why considered**: Smaller, reviewable diffs; no new concept.
- **Why rejected**: Three patches to three key derivations is three chances to
  miss a fourth layer — and independent-failure composition is precisely what
  produced the incident. One digest, one propagation path, one place to be
  right.

### 5. Document a `--force`/"ignore caches" flag as the remedy
- **Why considered**: Zero code change; the operator already had one.
- **Why rejected**: Making correctness depend on the operator remembering a flag
  converts a silent wrong answer into a documented wrong answer. The flag
  remains available for other reasons, but it is not a fix. `--force` and
  `--fresh` keep their existing meanings.

### 6. Treat `git_metadata = true` as non-volatile
- **Why considered**: Avoids the "no reuse at all" cost for such plans.
- **Why rejected**: These sources always clone from upstream, so the extracted
  content is unknowable until after the clone. Any key that claimed otherwise
  would be a declaration-derived key, which is the defect this ADR exists to
  remove.

---

## Consequences

### Positive

- A byte change in any local plan source invalidates charge, the CAS, and every
  stage checkpoint, by construction. "I changed it but the machine did not" is
  no longer representable.
- Local sources become content-addressed build inputs, so the build key is a
  truthful description of what was built — consistent with ADR-0048's
  content-addressed storage and ADR-0023's ledger.
- The first stage's hash now has a root. A checkpoint can no longer be
  "complete" with respect to inputs it never considered.
- One concept replaces three ad-hoc derivations, and dead `build_key` plumbing
  is gone.

### Negative Consequences & Trade-offs

- Local sources are read and SHA-256'd at the start of every build. Local
  sources are hand-authored snippets (units, configs, patches), so the cost is
  bounded by their size and paid once per build, not per stage.
  *Mitigation*: the digest is resolved before the marker check, so an unchanged
  tree still short-circuits all subsequent work.
- Plans containing a volatile source lose CAS and stage reuse entirely.
  *Mitigation*: this applies only to `git_metadata = true` plans (which already
  always hit the network) and to the very first build of a `SKIP` source.
- A `SKIP` source's build key changes once, between its first and second build.
  *Mitigation*: `SKIP` is a development affordance, not a distribution input.
- Marker and checkpoint on-disk formats gain a field; existing forge roots
  re-materialize once after upgrading. *Mitigation*: the digest format version
  makes this a deliberate, single, bounded invalidation rather than a silent
  mismatch.
- **Boundary — source drift is a separate decision.** `wright upgrade` decides
  whether to *act* on a plan from the registry's recorded provenance, which is
  the subject of [ADR-0055](0055-plan-drift-covers-bundled-sources.md). This
  ADR guarantees that, once a plan is selected for rebuild (any `plan.toml`
  change, any bundled-source change, `--force`, or a dependency's rebuild), the
  new source content is used.

---

## Links

- Related ADRs: [ADR-0004](0004-no-magic-behavior.md),
  [ADR-0023](0023-parts-as-maintenance-ledger.md),
  [ADR-0024](0024-workdir-source-names-are-original-basenames.md),
  [ADR-0033](0033-plan-source-snapshots.md),
  [ADR-0038](0038-git-source-snapshots.md),
  [ADR-0048](0048-action-graph-scheduler-and-domain-storage.md)
- Implementation: `crates/wright-actions/src/foundry/charge/identity.rs`,
  `crates/wright-actions/src/foundry/mod.rs`,
  `crates/wright-actions/src/foundry/forge/pipeline.rs`,
  `crates/wright-actions/src/foundry/checkpoint.rs`,
  `crates/wright-actions/src/operations/install/fingerprints.rs`
- Narrative: [Checkpoint Recovery](../explanation/checkpoint-recovery.md),
  [Plan Build](../explanation/plan-build.md),
  [Delivery Recovery](../explanation/delivery-recovery.md)

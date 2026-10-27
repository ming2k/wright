---
id: ADR-0055
title: "Plan Drift Covers the Whole Plan, Not Just plan.toml"
status: accepted
date: 2026-10-26
scope: core/registry, core/engine
superseded_by: null
negative_knowledge: true
---

# 0055. Plan Drift Covers the Whole Plan, Not Just plan.toml

- Status: Accepted
- Date: 2026-10-26
- Deciders: Core Engineering Team
- Architecturally consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

ADR-0054 gave a plan's sources a content address and made every *cache key*
derive from it, so an edited `[[sources]] type = "local"` file can no longer be
masked by a stale marker, a stale CAS entry, or a stale checkpoint.

One layer above the caches was left alone, and it is the layer a user meets
first. `wright upgrade <plan>` decides whether to act by asking
`plan_is_outdated`, which compared the version triple and
`plans.plan_checksum` — the SHA-256 of `plan.toml` alone. A plan is a
*directory*: `local` sources live beside `plan.toml` and are build inputs
(ADR-0017's "plan source" is the directory, not one file). Editing only a
bundled file therefore left the plan reported as "already up to date", and
`wright upgrade` did nothing at all.

That is the same user-visible outcome ADR-0054 removed from the build path,
reached by a different route:

```
edit plans/mypkg/mypkg.service
$ sudo wright upgrade mypkg
specified plans are already up to date      # plan.toml is byte-identical
$ sudo wright upgrade all
all plans are up to date
```

The caches were no longer lying, but the *selection* decision still was, and
`--force` was the only way through. Correctness that depends on an operator
remembering a flag is not correctness.

The information needed to fix it already existed but was not content-addressed:
every part carries `[provenance] source_checksums` (ADR-0023), one descriptive
line per source, persisted into `plans.source_checksums` at deploy
registration and rebuildable from part archives by `db reindex` (ADR-0043).
For a `local` source the line was `local <path>` — a declaration, with nothing
to compare. ADR-0054's own invariant said no cache key may hash a declaration;
the audit ledger was still doing exactly that.

## Decision Drivers

- **A cache key and a drift decision are the same question.** Both ask "are the
  inputs the same as when this artefact was produced?". Answering it two ways
  guarantees they eventually disagree.
- **No truth by flag.** If an input changed, the system must say so without the
  operator passing `--force`.
- **Absent data is not evidence.** A provenance line with no content address
  cannot prove sameness, and must never be read as "no drift".
- **Reuse the ledger, do not grow it.** `source_checksums` exists, is already
  populated on every deploy, is already reindexable, and needs no migration.

## Decision

**Plan drift is measured over the whole plan: `plan.toml` by checksum, and every
source by content address.**

1. **A plan knows where it came from.** `PlanManifest::plan_dir` records the
   directory a manifest was loaded from (`None` for manifests parsed from a
   string). Plan-relative inputs resolve against it — never against an ambient
   working directory, and never against a caller-supplied path that might
   disagree. `resolve_sources` gives it precedence over its own `plan_dir`
   argument for exactly this reason.
2. **Provenance is content-addressed.** `source_provenance_lines` produces
   `http <url> sha256=<sum>`, `git <url> ref=<ref>`, and now
   `local <path> sha256=<file sum>`. It is the same function that feeds
   `SourceDigest`, so the ledger and the cache keys can never disagree about
   what a plan's sources hash to. A `local` source that cannot be resolved is an
   error, not a degraded line (`[INV-SRC-06]`).
3. **Drift is decided from the ledger.** `plan_is_outdated` (selection) and
   `wright doctor` (reporting) compare the recorded `source_checksums` with the
   lines the plan on disk would produce now. The comparison runs only after
   `plan_checksum` has already matched, so any difference it finds is a bundled
   source's content and nothing else.
4. **Uncomparable is not clean.** `PlanRecord.source_checksums` is
   `Option<Vec<String>>`: a `local` line without a `sha256=`, or a column this
   code did not write, yields `None` from the comparison. Neither selection nor
   the doctor report treats that as "up to date" — they fall back to the
   previous checksum-only behaviour rather than inventing an answer.

## Invariants

- **`[INV-DRIFT-01]` The Ledger Records Content, Not Declarations**: Every
  `local` entry in `.PARTINFO` `[provenance] source_checksums` MUST carry the
  SHA-256 of the file's bytes. A path-only line MUST NOT be written by a seal
  performed by this code.
- **`[INV-DRIFT-02]` Outdatedness Covers The Whole Plan**: A plan MUST be
  reported outdated when any of its sources has drifted, not only when
  `plan.toml` has. A plan whose bundled file changed while `plan.toml` did not
  MUST be selected by `wright upgrade` without `--force`.
- **`[INV-DRIFT-03]` Absent Evidence Is Never "No Drift"**: When recorded
  provenance is missing, unparseable, or not content-addressed, the comparison
  MUST abstain. An unknown MUST NOT be reported as "up to date" and MUST NOT be
  resolved by guessing an address.
- **`[INV-DRIFT-04]` A Plan Is Its Directory**: Plan-relative paths MUST
  resolve against the directory the manifest was loaded from
  (`PlanManifest::plan_dir`). Resolving them against the process working
  directory, or against a path supplied independently of the manifest, is
  prohibited.
- **`[INV-DRIFT-05]` One Derivation Of Source Content**: Build identity and
  audit provenance MUST be produced by the same source-hashing code. A second
  implementation of "what do this plan's sources hash to" MUST NOT exist
  (`[INV-SRC-01]`).

## Rejected Alternatives & Negative Knowledge

### 1. Fold bundled files into `plan_checksum`
- **Why considered**: Zero new fields; selection and the doctor would work
  unchanged.
- **Why rejected**: It redefines a persisted, archived, user-visible field and
  conflates two facts — "the plan text changed" and "a file beside it changed".
  It also breaks the drift *explanation*: `.PLANSRC` holds only the `plan.toml`
  text (ADR-0033), so the unified diff the doctor prints on drift would no
  longer correspond to the checksum that triggered it. Explanation would be
  traded for detection.

### 2. Require `--force` for local-source changes
- **Why considered**: No code at all; the flag already existed and the status
  quo was at least honest about doing nothing.
- **Why rejected**: Correctness conditional on remembering a flag is a bug with
  documentation attached. It also leaves two answers to one question (is this
  plan current?) depending on which command you run.

### 3. A new registry column or archive key for the source digest
- **Why considered**: The digest is the canonical identity; storing it directly
  looks simpler than parsing per-source lines.
- **Why rejected**: `source_checksums` already carries exactly this information,
  is already written on every deploy, and is already rebuilt from part archives
  by `db reindex` (ADR-0043). A new column needs an append-only migration and a
  new reindex path for no new capability. The digest additionally loses the
  per-source attribution that makes a drift report legible.

### 4. Compare only the local-source lines
- **Why considered**: Strictly minimal; local content is the only thing
  `plan_checksum` cannot see.
- **Why rejected**: It special-cases one source kind inside a general
  comparison and would silently ignore a future source kind whose declaration is
  not fully covered by the plan text. Comparing the whole line set is simpler,
  and — because it only runs once `plan_checksum` has matched — cannot report
  anything other than a bundled input's content.

### 5. Treat unparseable provenance as drift
- **Why considered**: Fail loud; a part whose provenance cannot be read is
  arguably not trustworthy.
- **Why rejected**: It would mark every plan sealed by an older binary as
  outdated, causing a system-wide rebuild to learn nothing. Abstaining and
  falling back to checksum-only behaviour is the honest reading: the tool says
  it cannot tell, rather than guessing in either direction.

## Consequences

### Positive

- "I changed it and the machine did not" is no longer representable at any
  layer: selection, charge, CAS, checkpoints, and staging all key on content.
- `wright doctor` can explain a class of drift it previously could not see, and
  `wright upgrade` converges a plan whose only change was a bundled file,
  without `--force`.
- The ledger and the cache keys share one source-hashing implementation, so an
  audit and a build can never disagree about a plan's inputs.
- `PlanManifest::plan_dir` removes an entire class of "resolved relative to
  whatever the process cwd happened to be" bugs.

### Negative Consequences & Trade-offs

- **A plan with a `local` source is now selected for rebuild more often.** Its
  content, not just its `plan.toml`, decides. *Mitigation*: this is the point,
  and the rebuild is exactly one forge of one plan.
- **Parts sealed before this change carry path-only `local` lines**, so drift
  cannot be detected for them until they are re-sealed. *Mitigation*:
  `[INV-DRIFT-03]` abstains rather than misreporting; the first re-seal
  (any `plan.toml` change or `--force`) upgrades the ledger.
- **`PlanManifest::plan_dir` must travel with the manifest.** A manifest
  constructed by hand and never given a directory cannot resolve plan-relative
  sources. *Mitigation*: that case fails closed with a message naming the
  source (`[INV-SRC-06]`), and every production path loads from a file.
- **`plans.source_checksums` is parsed, not schematised.** Drift comparison
  depends on the line format. *Mitigation*: `[INV-DRIFT-05]` keeps generation
  and consumption adjacent, and one test pins the format.

## Links

- Related ADRs: [ADR-0017](0017-plan-source-single-dep-truth.md),
  [ADR-0023](0023-parts-as-maintenance-ledger.md),
  [ADR-0033](0033-plan-source-snapshots.md),
  [ADR-0041](0041-file-backed-ledger.md),
  [ADR-0043](0043-registry-as-derived-index-and-maintenance-surface.md),
  [ADR-0054](0054-content-addressed-source-identity.md)
- Implementation: `crates/wright-actions/src/foundry/charge/identity.rs`
  (`source_provenance_lines`), `crates/wright-actions/src/seal/execute.rs`,
  `crates/wright-actions/src/operations/plan_drift.rs`,
  `crates/wright-actions/src/operations/upgrade.rs`,
  `crates/wright-actions/src/operations/doctor.rs`,
  `crates/wright-registry/src/database/plans.rs`,
  `crates/wright-plan/src/manifest.rs`
- Reference: [Local Part Inventory](../reference/local-inventory.md),
  [Database Design](../reference/database-design.md)

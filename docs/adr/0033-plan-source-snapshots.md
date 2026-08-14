# ADR-0033: Plan-Source Snapshots in Parts and the Registry

## Status

Accepted

Amended by [ADR-0041](0041-file-backed-ledger.md): the `plan_snapshots`
table described under **Persist** is replaced by snapshot files in the
ledger directory. Capture (`.PLANSRC`) and the read surfaces (`wright
doctor`, `wright plan`) are unchanged.

## Context

[ADR-0023](0023-parts-as-maintenance-ledger.md) gave every part descriptive
provenance, including `plan_checksum` — the SHA-256 of the plan source that
produced it. That closes the "did the plan change?" question: `wright doctor`
compares the recorded checksum against the current plan file and reports
drift. It does not close the next two questions a maintenance ledger must
answer:

- **What changed?** A checksum mismatch with no recorded content is a dead
  end. Once the plan file has been edited (or deleted), there is nothing to
  diff against and nothing to learn from the mismatch.
- **What exactly produced this part?** The checksum identifies the plan
  content but cannot reproduce it. If the source tree is lost or rewritten,
  the exact inputs behind an installed part are unrecoverable.

ADR-0017 already treats "the plan source can be reconstructed from a part
archive" as a property worth preserving, and ADR-0023 calls for foreign
parts to be *auditable*. Both point at recording content, not just identity.

Two constraints from existing decisions bound the shape of any answer. The
registry records facts and never enforces them (ADR-0016), so snapshots are
audit data that deploy, resolve, and remove must never consult — those paths
stay driven by the installed-state database. And the plan source is the
single dependency truth (ADR-0017), so the snapshot must be the raw source,
not a derived rendering that could drift from it.

## Decision

The exact `plan.toml` text a part was sealed from is captured, carried, and
persisted:

1. **Capture.** `PlanManifest::from_file` retains the raw plan text as
   `plan_source`, covering the same bytes `plan_checksum` hashes (the
   `mvp.toml` overlay is excluded from both). Manifests parsed from a string
   have no source and seal no snapshot.
2. **Carry.** The seal step embeds the text as a `.PLANSRC` member at the
   archive root, alongside `.PARTINFO`/`.FILELIST`/`.HOOKS`. It is metadata,
   not payload: excluded from `.FILELIST` and from deploy-time file
   collection, and removed from the staging tree after sealing. The member
   is optional by contract — readers treat its absence like pre-ADR-0023
   provenance. Because the snapshot lives inside the archive, CAS-cached and
   foreign parts carry it with no extra machinery.
3. **Persist.** Deploy and upgrade registration insert the snapshot into a
   new `plan_snapshots` table keyed by the SHA-256 of the text — the same
   value `plans.plan_checksum` records. Keying by content deduplicates
   re-seals of unchanged plans (`INSERT OR IGNORE`) and retains every
   distinct version ever deployed. There is no foreign key; rows whose
   checksum no plan references anymore are retained deliberately, because
   the ledger keeps history.
4. **Surface.** On drift, `wright doctor` prints a unified diff between the
   recorded snapshot and the current plan source beneath the checksum line.
   A new query command, `wright plan <TARGET>`, prints the recorded source
   as exact bytes (round-trippable to disk) with `--json` for the structured
   shape. Both are read paths over audit data; no state-changing operation
   reads snapshots.

## Alternatives

**Checksum-only provenance (status quo).** Rejected — detection without
content leaves the ledger's audit questions unanswered exactly when they
matter (source edited or gone).

**A derived configuration mapping instead of raw source.** Rejected — a
rendering is a second fact that must stay synchronized with the plan, which
is the drift pattern ADR-0017 exists to eliminate. A raw copy cannot drift
from itself.

**Store the source on the `plans` row.** Rejected — re-registering a plan
overwrites the row, destroying the previous snapshot. A checksum-keyed
table keeps every version and matches how the value is actually referenced.

**Snapshot the fetched sources as well.** Rejected — full reproducibility
inputs are a different problem at a different scale; `source_checksums`
already records them descriptively (ADR-0023), and the CAS covers rebuild
reuse.

**No retrieval command; rely on archive extraction.** Rejected — the
database is the ledger's query surface, and requiring operators to locate
and untar an archive to read a few kilobytes of text makes the feature
effectively absent.

## Consequences

### Positive

- Drift detection becomes drift *explanation*: `wright doctor` shows what
  changed, not just that something did.
- The exact plan content behind any deployed part remains recoverable after
  the source is edited or deleted, via `wright plan` or the archive itself.
- Foreign parts become auditable in full, strengthening the ADR-0023 stance
  without any trust machinery.
- Round-tripping (ADR-0017) is preserved: a part archive once again
  contains everything needed to reconstruct its plan source.

### Negative

- Every output archive of a multi-output plan carries its own copy of the
  plan text. The cost is a few kilobytes per archive.
- `plan_snapshots` grows monotonically. Rows are small text; retention is a
  ledger feature, and a future `wright prune` extension can revisit it.
- Parts sealed from string-parsed manifests (tests, synthetic manifests)
  carry no snapshot; all production seal paths load plans from files, so
  this does not affect real installs.

## Related

- [ADR-0016](0016-advisory-runtime-dependencies.md) — the "record facts, do
  not enforce" stance snapshots follow.
- [ADR-0017](0017-plan-source-single-dep-truth.md) — plan source as the
  single truth; snapshots preserve it verbatim.
- [ADR-0023](0023-parts-as-maintenance-ledger.md) — the ledger model and
  provenance this ADR extends from identity to content.
- [Local Part Inventory](../reference/local-inventory.md) — archive member
  reference.
- [Database Design](../reference/database-design.md) — `plan_snapshots`
  schema.

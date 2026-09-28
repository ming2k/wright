---
id: ADR-0049
title: "Plan Output Evolution, Automated Retirement, and Resilient Storage Transactions"
status: accepted
date: 2026-09-27
scope: core/engine
superseded_by: null
negative_knowledge: true
---

# 0049. Plan Output Evolution, Automated Retirement, and Resilient Storage Transactions

- Status: Accepted
- Date: 2026-09-27
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

Wright enforces strict correctness invariants to ensure reproducibility and system integrity:
1. **Plan-Level Atomicity (All-Outputs Lockstep)**: A plan originates from a single source specification; all deployed outputs (parts) belonging to that plan must share the same revision (`epoch`, `version`, `release`, `arch`).
2. **Deterministic Transactional Rollback**: Mutating operations (install, upgrade, remove) record journaled intents and preserve backups under `/var/lib/wright/rollback` before committing live filesystem mutations.

However, in rolling release distributions and production Linux deployments, two architectural tensions and failure modes emerged under real-world software maintenance:

### 1. The Output Evolution Deadlock (Missing Retirement Semantics)
Software components naturally evolve over time: subpackages are split, merged, renamed, or retired (clean-break restructuring). For example, upstream `optics` might deprecate and drop `flux-text` in favor of a consolidated `glyph` output.

In Wright's historical pre-deploy validation (`validate_plan_output_batches`), any installed output of plan $P$ missing from the incoming deployment batch was treated as an illegal partial upgrade:
```text
cannot deploy plan 'P' <new_rev> while deployed output(s) from <old_rev> would remain: <stale>; deploy those outputs in the same batch or use wright install P
```
Because the new plan revision no longer declared or built `<stale>`, running `wright install P` could never produce `<stale>`. 

Furthermore, when operators attempted to manually unblock the system using `wright remove -f <stale>`, the removal planner (`plan_removal`) entirely ignored the `--force` flag during dependency closure validation, hard-failing if any installed package depended on `<stale>`. This created an **irrecoverable system deadlock**:
- The plan could not be upgraded because the stale output was absent from the new revision.
- The stale output could not be uninstalled because downstream dependents blocked removal, even with `-f`.
- The operator's only recourse was manual SQL tampering with the registry database.

### 2. Cross-Device Symlink Transaction Failure (`EXDEV` Dereferencing)
Modern Linux systems adhere to storage partitioning standards where the root filesystem (`/`) and state/data directories (`/var`) reside on separate mount points or Btrfs subvolumes.

When moving files from `/usr/lib` to the backup store under `/var/lib/wright/rollback`, the kernel's `rename(2)` returns `EXDEV` (Cross-device link). Historical transactional code in `move_path` caught `EXDEV` and fell back to `tokio::fs::copy`. 

However, `tokio::fs::copy` dereferences symbolic links. In Linux dynamic library layouts (e.g. `libflux_text.so.0.0 -> libflux_text.so.0.0.1`), moving the physical shared object first leaves the versioned symlink dangling. When `fs::copy` subsequently encountered the dangling symlink, it failed with `ENOENT (os error 2)`, causing the entire upgrade transaction to fail catastrophically during backup preparation.

---

## Decision Drivers

- **Declarative Package Evolution**: Enable seamless rolling upgrades across upstream package splits, merges, and clean-breaks without manual operator intervention.
- **Strict Plan-Level Revision Invariance**: Prevent partial upgrades and ensure that active outputs of a plan remain strictly synchronized on the same revision.
- **Fail-Safe Force Escape Hatch**: Guarantee that `--force` on `wright remove` truly overrides dependency blocking, preventing administrative deadlocks.
- **POSIX VFS and Storage Topology Resiliency**: Provide robust transactional behavior across disparate filesystems, mount points, and dangling symlinks.

---

## Considered Options

- **Option 1: Semantic Stale Output Classification with Automated Pre-Deploy Retirement + True Force Removal + Symlink-Aware Cross-Device Backup (Chosen)**
- **Option 2: Strict Manual Interventions Only**: Preserve the rigid refusal and require operators to manually remove or rename stale outputs before upgrading.
- **Option 3: Relaxed Revision Consistency**: Allow different outputs from the same plan to coexist across different revisions indefinitely.

---

## Decision Outcome

Chosen Option: **Option 1**.

### 1. Semantic Output Batch Validation and Automated Retirement
In `crates/wright-actions/src/transaction/deploy.rs`, `validate_plan_output_batches` is enhanced from a rigid binary blocker into a semantic classifier:
1. **On-Demand Manifest Inspection**: When stale outputs are detected ($O_{\text{installed}} \setminus O_{\text{incoming}} \neq \emptyset$), the deployer streams `.PLANSRC` from candidate archives without full extraction via `read_archive_plansrc`.
2. **Classification Logic**:
   - **Declared Supersession (`replaces`)**: If a stale output is declared in any candidate's `partinfo.replaces`, it is marked for retirement.
   - **Source Manifest Deprecation**: If `.PLANSRC` reveals that the new plan revision no longer declares the output, it is marked as a retired stale output.
   - **Active Output Omission**: If the output is still declared by the plan and not replaced, it represents an illegal partial upgrade and deployment is aborted with actionable guidance.
3. **Pre-Deploy Transactional Retirement**: Before deploying new outputs, all approved retirements are uninstalled via `remove_part`. Files are backed up, empty directories cleaned, and registry records purged.
4. **Dependent Safeguards**: If a retired output has downstream external dependents that are not part of the incoming batch and it was not superseded via `replaces`, retirement requires `--force`.

### 2. Functional Force Removal in `plan_removal`
`plan_removal` in `crates/wright-actions/src/operations/remove.rs` now takes `force: bool`. When `force` is asserted:
- Dependent blocking validation emits warning events (`remove.forced`) instead of returning `WrightError::DependencyError`.
- Operators can decisively untangle dependency deadlocks from the CLI using `wright remove -f`.

### 3. Symlink-Preserving Cross-Device Transaction Engine
In `crates/wright-actions/src/transaction/fs_tx.rs`, `move_path` inspects symlink metadata prior to copying across `EXDEV` boundaries:
```rust
let meta = tokio::fs::symlink_metadata(src).await?;
if meta.is_symlink() {
    let target = tokio::fs::read_link(src).await?;
    tokio::fs::symlink(&target, dst).await?;
    let _ = tokio::fs::remove_file(src).await;
    return Ok(());
}
```
This preserves the exact symlink structure into the rollback store without attempting to read the (potentially displaced) underlying file, eliminating `ENOENT` crashes.

---

## Positive Consequences

- **Unattended Rolling Upgrades**: Systems upgrade smoothly across major upstream refactorings without aborting or leaving orphan artifacts.
- **Zero Orphaned Files**: Deprecated subpackages have their physical files and symlinks cleanly deleted from the live root upon plan upgrade.
- **Deadlock Immunity**: Administrative operators can reliably remove stubborn packages with `--force`.
- **Cross-Mount Safety**: Fully compatible with advanced Linux partition layouts (`/` on root subvolume, `/var` on separate subvolume/disk).

## Negative Consequences

- **Minor Pre-Deploy Latency**: Streaming `.PLANSRC` from `.wright.tar.zst` headers incurs a negligible decompression pass over archive metadata when stale outputs are present.
- **Irreversible Dependent Disruption Under Force**: Using `--force` to retire or remove parts with active dependents can leave downstream binaries without required shared libraries until rebuilt.

---

## Rejected Alternatives & Negative Knowledge

### Why Option 2 (Strict Manual Interventions Only) Was Discarded
Option 2 preserves the simplistic assumption that plan output topographies are immutable across versions. In practice, upstream projects frequently reorganize submodules. Forcing operators to manually track down and remove retired subpackages turns routine system upgrades (`wright upgrade all`) into high-friction manual interventions.

### Why Option 3 (Relaxed Revision Consistency) Was Discarded
Allowing different outputs of a plan to diverge across revisions violates Wright's core principle of reproducibility. A split package (such as `gcc` and `libstdc++`, or `python` and `python-stdlib`) compiled from source requires synchronized headers and runtimes. Permitting mixed revisions leads to undefined dynamic linker behavior, subtle symbol mismatches, and unreproducible systems.

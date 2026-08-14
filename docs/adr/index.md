# Architecture Decision Records

| ADR | Decision | Status |
|-----|----------|--------|
| [ADR-0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [ADR-0002](0002-wave-by-wave-install.md) | Wave-by-wave install instead of one big install | Accepted |
| [ADR-0003](0003-default-resolution-policy.md) | Default resolution policy for install | Accepted |
| [ADR-0004](0004-no-magic-behavior.md) | No implicit magic behavior | Accepted |
| [ADR-0005](0005-two-database-design.md) | Two-database design (installed + archive) | Superseded by ADR-0030 |
| [ADR-0006](0006-mvp-two-pass-build.md) | MVP two-pass build for dependency cycles | Accepted |
| [ADR-0007](0007-usrmerge-and-sbin-merge.md) | usrmerge and sbin merged into bin | Accepted |
| [ADR-0008](0008-no-dev-splitting.md) | No -dev splitting for personal distributions | Accepted |
| [ADR-0009](0009-separate-plan-output-dependencies.md) | Separate plan-level and output-level dependencies | Accepted |
| [ADR-0010](0010-pre-copied-sysroot-isolation.md) | Pre-copied read-only sysroot instead of OverlayFS | Superseded by ADR-0012 |
| [ADR-0011](0011-plan-name-only-dep-all-outputs.md) | Plan-name-only dependency references resolve to all outputs | Accepted |
| [ADR-0012](0012-overlayfs-per-task-upper.md) | OverlayFS with per-task writable upper layers | Superseded by ADR-0013 |
| [ADR-0013](0013-multi-lowerdir-isolation.md) | Multi-lowerdir OverlayFS isolation | Accepted |
| [ADR-0014](0014-launch-and-pack-format.md) | `wright launch` and the pack format | Superseded by ADR-0015 |
| [ADR-0015](0015-folio-manifest-replaces-pack.md) | Folio manifest replaces pack format | Accepted (format details superseded by ADR-0031) |
| [ADR-0016](0016-advisory-runtime-dependencies.md) | Runtime dependencies are advisory, not enforced | Accepted |
| [ADR-0017](0017-plan-source-single-dep-truth.md) | Plan source as single dep truth + ELF lint | Accepted |
| [ADR-0018](0018-unified-cli-porcelain-plumbing.md) | Unified CLI with porcelain–plumbing separation and convergent file layout | Accepted (file layout superseded by ADR-0020) |
| [ADR-0019](0019-cas-delivery-recovery.md) | Two-layer CAS + WAL recovery for delivery | Accepted |
| [ADR-0020](0020-merge-cli-and-commands-directories.md) | Merge `src/cli/` and `src/commands/` into a single directory | Accepted |
| [ADR-0021](0021-cargo-style-span-driven-output.md) | Cargo-style span-driven CLI output (+ companion correctness fixes) | Accepted |
| [ADR-0022](0022-git-fetch-via-libgit2-no-system-git.md) | Git source fetching via libgit2, never the system `git` | Superseded by ADR-0035 |
| [ADR-0023](0023-parts-as-maintenance-ledger.md) | Parts are maintenance-ledger artifacts, not distribution products | Accepted |
| [ADR-0024](0024-workdir-source-names-are-original-basenames.md) | Work-directory source names are original basenames | Accepted |
| [ADR-0025](0025-incremental-cargo-workspace.md) | Incremental Cargo workspace with stable internal crate boundaries | Accepted |
| [ADR-0026](0026-workspace-crate-boundaries.md) | Workspace boundaries for CLI, engine, state, part, plan, and model | Superseded by ADR-0029 |
| [ADR-0027](0027-isolation-fails-closed.md) | Isolation fails closed | Accepted |
| [ADR-0028](0028-single-threaded-isolation-helper.md) | Namespace setup runs in a single-threaded helper process | Accepted |
| [ADR-0029](0029-engine-owned-boundary-mapping.md) | Engine-owned mapping between sibling workspace crates | Accepted |
| [ADR-0030](0030-single-database.md) | Single database for system state | Accepted |
| [ADR-0031](0031-folio-manifest-amendments.md) | Folio manifest amendments (hooks, `<name>.toml`, no `[config]`/`arch`) | Accepted |
| [ADR-0032](0032-cli-surface-consistency.md) | CLI surface consistency conventions | Accepted (`-c` row and dry-run inversion superseded by ADR-0040) |
| [ADR-0033](0033-plan-source-snapshots.md) | Plan-source snapshots in parts and the registry | Accepted (persistence amended by ADR-0041) |
| [ADR-0034](0034-plan-output-namespaces-and-part-layout.md) | Plan/output namespaces and plan-qualified part layout | Accepted |
| [ADR-0035](0035-git-fetch-via-gitoxide.md) | Git source fetching via gitoxide (gix) | Accepted |
| [ADR-0036](0036-merged-base-stage-overlays.md) | Merged-base stage layers with sandbox-mounted stage overlays | Superseded by ADR-0037 |
| [ADR-0037](0037-real-directory-stage-trees.md) | Real directory stage working trees instead of stage overlays | Accepted |
| [ADR-0038](0038-git-source-snapshots.md) | Git sources cached as tree snapshots | Accepted |
| [ADR-0039](0039-batch-failure-settlement.md) | Batch failure settlement instead of fail-fast | Accepted |
| [ADR-0040](0040-clean-prune-consolidation-and-fresh-flag.md) | Consolidate maintenance deletion into `clean`; rename the from-scratch forge flag to `--fresh` | Accepted |
| [ADR-0041](0041-file-backed-ledger.md) | File-backed ledger: build records, snapshot files, and `.BUILDINFO` | Accepted |

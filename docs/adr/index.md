# Architecture Decision Records

| ADR | Decision | Status |
|-----|----------|--------|
| [ADR-0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [ADR-0002](0002-wave-by-wave-install.md) | Wave-by-wave install instead of one big install | Accepted |
| [ADR-0003](0003-default-resolution-policy.md) | Default resolution policy for install | Accepted |
| [ADR-0004](0004-no-magic-behavior.md) | No implicit magic behavior | Accepted |
| [ADR-0005](0005-two-database-design.md) | Two-database design (installed + archive) | Accepted |
| [ADR-0006](0006-mvp-two-pass-build.md) | MVP two-pass build for dependency cycles | Accepted |
| [ADR-0007](0007-usrmerge-and-sbin-merge.md) | usrmerge and sbin merged into bin | Accepted |
| [ADR-0008](0008-no-dev-splitting.md) | No -dev splitting for personal distributions | Accepted |
| [ADR-0009](0009-separate-plan-output-dependencies.md) | Separate plan-level and output-level dependencies | Accepted |
| [ADR-0010](0010-pre-copied-sysroot-isolation.md) | Pre-copied read-only sysroot instead of OverlayFS | Superseded by ADR-0012 |
| [ADR-0011](0011-plan-name-only-dep-all-outputs.md) | Plan-name-only dependency references resolve to all outputs | Accepted |
| [ADR-0012](0012-overlayfs-per-task-upper.md) | OverlayFS with per-task writable upper layers | Superseded by ADR-0013 |
| [ADR-0013](0013-multi-lowerdir-isolation.md) | Multi-lowerdir OverlayFS isolation | Accepted |
| [ADR-0014](0014-launch-and-pack-format.md) | `wright launch` and the pack format | Superseded by ADR-0015 |
| [ADR-0015](0015-folio-manifest-replaces-pack.md) | Folio manifest replaces pack format | Accepted |
| [ADR-0016](0016-advisory-runtime-dependencies.md) | Runtime dependencies are advisory, not enforced | Accepted |
| [ADR-0017](0017-plan-source-single-dep-truth.md) | Plan source as single dep truth + ELF lint | Accepted |
| [ADR-0018](0018-unified-cli-porcelain-plumbing.md) | Unified CLI with porcelain–plumbing separation and convergent file layout | Accepted (file layout superseded by ADR-0020) |
| [ADR-0019](0019-cas-delivery-recovery.md) | Two-layer CAS + WAL recovery for delivery | Accepted |
| [ADR-0020](0020-merge-cli-and-commands-directories.md) | Merge `src/cli/` and `src/commands/` into a single directory | Accepted |
| [ADR-0021](0021-cargo-style-span-driven-output.md) | Cargo-style span-driven CLI output (+ companion correctness fixes) | Accepted |
| [ADR-0022](0022-git-fetch-via-libgit2-no-system-git.md) | Git source fetching via libgit2, never the system `git` | Accepted |
| [ADR-0023](0023-parts-as-maintenance-ledger.md) | Parts are maintenance-ledger artifacts, not distribution products | Accepted |
| [ADR-0024](0024-workdir-source-names-are-original-basenames.md) | Work-directory source names are original basenames | Accepted |

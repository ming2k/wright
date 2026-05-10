# Architecture Decision Records

| ADR | Decision | Status |
|-----|----------|--------|
| [ADR-0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [ADR-0002](0002-wave-by-wave-install.md) | Wave-by-wave install instead of one big install | Accepted |
| [ADR-0003](0003-default-resolution-policy.md) | Default resolution policy for apply | Accepted |
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
| [ADR-0015](0015-group-manifest-replaces-pack.md) | Group manifest replaces pack format | Accepted |
| [ADR-0016](0016-advisory-runtime-dependencies.md) | Runtime dependencies are advisory, not enforced | Accepted |
| [ADR-0017](0017-plan-source-single-dep-truth.md) | Plan source as single dep truth + ELF lint | Accepted |

# Changelog

## [1.0.1] - 2026-02-20

### Fixes
- Add `-c` short alias for `--clean` flag in `wbuild run`
- Fix spurious WARN on archive root entry during package creation
- Track `Cargo.lock` for reproducible builds (removed from `.gitignore`)

## [1.0.0] - 2026-02-20

### Features
- Declarative TOML-based package plans with configure / compile / package lifecycle stages
- Sandboxed builds using Linux namespaces (mount, PID, network isolation)
- Split-package support for producing multiple output packages from a single build
- Bootstrap mode for building the initial system toolchain
- Git source support alongside HTTP archive downloads
- Dependency resolution with build / runtime / link classification
- `replaces` and `conflicts` fields for package compatibility management
- `doctor` subcommand for system health diagnostics
- Stage-level exec (`wbuild run <pkg> --stage <stage>`) for targeted rebuilds
- Resource limits on build processes to prevent runaway builds
- SHA-256 checksum verification for downloaded sources
- Archive support: `.tar.gz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`
- Symlink-aware tar packaging (archives symlinks as symlinks, not followed)
- Special file handling in archives (FIFO, char/block devices)
- Progress indicators for downloads and package operations
- Structured logging with `RUST_LOG` / `--log-level` control
- SQLite-backed package database

### Fixes
- Resolved empty root-entry warning during tar archive creation
- Fixed unsafe archive path detection (empty path vs. path traversal)
- Fixed URI name substitution for packages with version-templated URLs
- Fixed duplicate/conflicting file handling across split packages
- Mitigated potential resource exhaustion in allocation paths
- Correct `BUILD_DIR` remapping inside sandboxed environments

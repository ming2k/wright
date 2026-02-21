# Changelog

## [1.1.2] - 2026-02-21

### Fixes
- Fix test call to `Builder::build` missing `force` and `nproc_per_worker` arguments introduced in 1.1.1

## [1.1.1] - 2026-02-21

### Fixes
- Add `.max(1)` guard to NPROC resolution for all build types (defensive; scheduler share was always ≥ 1 in practice)
- Add per-plan `jobs` cap in `plan.toml [options]` applied after `build_type` modifier and global cap
- Fix cli-reference.md output table: multi-worker with explicit `--verbose` correctly documented as echoed (may interleave), not captured
- Rewrite resource-allocation.md: three-layer model, semantic alias clarification for `make`/`rust`/`custom` build types, `[options.env]` substitution behaviour, NPROC resolution as explicit computation steps

## [1.1.0] - 2026-02-21

### Features
- Replace numeric `jobs` field in `plan.toml` with semantic `build_type` label (`default`, `make`, `rust`, `go`, `heavy`, `serial`, `custom`)
- Add `[options.env]` for injecting package-wide environment variables into all lifecycle stages
- Scheduler now dynamically derives `$NPROC` per active worker (`total_cpus / active_workers`) so compiler concurrency self-adjusts as the dependency graph fans out or collapses
- `build_type = "go"` auto-injects `GOFLAGS` and `GOMAXPROCS`; `build_type = "heavy"` halves the thread share to cap RAM pressure; `build_type = "serial"` forces single-threaded builds

## [1.0.2] - 2026-02-20

### Fixes
- Fix `ETXTBSY` error when installing or rolling back a package that replaces a running executable

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

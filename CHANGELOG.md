# Changelog

## [Unreleased]

## [1.2.6] - 2026-02-22

### Features
- Add `wbuild run --skip-check` to skip only the lifecycle `check` stage while still running a full build pipeline (including fetch/verify/extract), without requiring `--stage` partial-build mode.

### Fixes
- Config files declared in `[backup]` now create `<path>.wnew` only when the live file already exists during upgrade. If the config path does not exist yet, the new file is installed directly to `<path>`.
- Dockyard CPU budgets are now partitioned fairly across each launch wave, avoiding misleading same-wave allocations like `16`, `8`, `5` that summed above the host CPU count.

## [1.2.5] - 2026-02-22

### Changes
- CPU scheduling default now uses all available CPUs when `[build].max_cpus` is unset (instead of implicitly reserving 4 cores for the OS). The dockyard status line no longer prints the "reserved 4 for OS" note.

### Features
- Add git fetch progress logging for `git+` sources: long fetches now emit transfer milestones (10% increments) so builds do not appear stalled during remote object downloads.

## [1.2.4] - 2026-02-22

### Features
- Layered config merging: all `wright.toml` files that exist (system `/etc/wright/`, user XDG, project-local `./`) are now merged in ascending priority order. Higher-priority files only need to set the keys they want to override; remaining keys are inherited from the layer below. The `--config` flag continues to bypass layering and load a single file as-is.
- Config file protection on upgrade: files declared in `[backup]` are never overwritten during an upgrade. The new package default is always written alongside as `<path>.wnew` with a warning so the user can diff and merge at their own pace. Files not declared in `[backup]` are overwritten directly as before.

### Fixes
- Fix `update_hashes` crash when a `git+` URI is listed in sources: the URI was passed directly to reqwest (which doesn't understand the `git+` scheme) instead of being skipped with `SKIP`.
- Fix git source cache directory name collision: repos sharing the same last URL path segment (e.g. `org-a/mylib.git` and `org-b/mylib.git`) now get distinct cache directories via a `<stem>-<8char-url-hash>` naming scheme.

## [1.2.3] - 2026-02-22

### Features
- FHS validation after the `package` stage: every file and symlink in `$PKG_DIR` is checked against the distribution's merged-usr path whitelist before the archive is created. Violations produce a `ValidationError` with a clear hint (e.g. "install to /usr/bin"). Absolute symlink targets are also validated. Set `[options] skip_fhs_check = true` to bypass for edge cases such as kernel modules.

## [1.2.2] - 2026-02-21

### Changes
- Remove `optional` field from lifecycle stages. Stages either run and must pass, or are skipped via `--stage`. Use `--stage` to omit the `check` stage instead of silently ignoring test failures.

## [1.2.1] - 2026-02-21

### Changes
- Replace `--until` and `--only` lifecycle flags with a unified `--stage` flag that accepts multiple values (e.g. `--stage check --stage package`). Empty `--stage` runs the full pipeline; one or more `--stage` values run exactly those stages in pipeline order, skipping fetch/verify/extract (requires a previous full build).
- `wbuild fetch` now correctly stops after source extraction without running lifecycle stages.

## [1.2.0] - 2026-02-21

### Features
- Rename "sandbox" isolation environment to "dockyard" throughout codebase, config, TOML fields, and docs
- Rename "worker" concurrency concept to "dockyard" for consistency (`workers` → `dockyards`, `nproc_per_worker` → `nproc_per_dockyard`)
- Add `max_cpus` config field to hard-cap total CPU cores used; defaults to `available_cpus - 4` (minimum 1) to reserve headroom for the OS
- Print resource info at build start: active dockyard count, total/available CPUs, and per-dockyard CPU share
- Update all documentation for new terminology and CPU allocation model

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

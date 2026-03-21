# Changelog

## [Unreleased]

## [1.10.0] - 2026-03-21

### Breaking Changes
- **Origin enum replaces install_reason**: the `install_reason` database column and field have been replaced by a typed `Origin` enum with three variants: `manual` (user ran `wright install`), `build` (installed via `wbuild -i`), and `dependency` (auto-resolved). The DB column is renamed from `install_reason` to `origin`. Run `tools/migrate_origin.py` to migrate existing databases.

### Changes
- **Promotion rules**: origin can only be promoted upward (`dependency → build → manual`), never downgraded. `wbuild -i` no longer overwrites a `manual` origin.
- **`wbuild run --resume`**: add `--resume` flag to skip parts that were already successfully built and installed in a previous session.

## [1.9.0] - 2026-03-21

### Breaking Changes
- **Merge `wbuild deps` into `wbuild resolve --tree`**: the `wbuild deps` subcommand has been removed. Use `wbuild resolve <TARGET> --tree` (with optional `--depth=<N>`) for static dependency tree visualization from hold-tree `plan.toml` files. The `--tree` flag is incompatible with `--self`, `--deps`, and `--dependents`.

## [1.8.3] - 2026-03-21

### Changes
- **Log style unification**: remove bracket labels from all log output, use lowercase natural-language action labels (`build`, `relink`, `rebuild`, `build:mvp`, `build:full`) with depth annotations in the scheduling plan.
- **SendError panic fix**: gracefully handle dropped channel receiver when a build fails, preventing worker thread panics on early exit.

## [1.8.2] - 2026-03-21

### Changes
- **Construction Plan logging cleanup**: unify plan labels around scheduled actions (`[BUILD]`, `[RELINK]`, `[REBUILD]`, `[BUILD:MVP]`, `[BUILD:FULL]`) and present the plan in stable dependency order.
- **Build log wording alignment**: separate explanatory `INFO` logs from the plan summary so dependency expansion, cycle resolution, and MVP execution messages use consistent scheduling language.

## [1.8.1] - 2026-03-21

### Changes
- **MVP documentation and messaging**: update error messages, CLI help, and docs to mention sibling `mvp.toml` as an alternative to inline `[mvp.dependencies]` for declaring acyclic MVP dependency sets.

## [1.8.0] - 2026-03-18

### Breaking Changes
- **Part relations moved to output level**: `[relations]` section removed. `replaces`, `conflicts`, and `provides` are now declared per-output in `[output]` (main part) or `[output.<name>]` (sub-part). This enables multi-output plans where each sub-part declares its own relations independently.
- **Legacy compatibility code removed**: historical rejection tests and special-case error messages for obsolete syntax (`[lifecycle.part]`, `[split.*]`, `[install_scripts]`, `[backup]`, relations in `[dependencies]`) have been removed. Invalid fields are still rejected by `deny_unknown_fields`.

## [1.7.10] - 2026-03-18

### Changes
- **Archive metadata completeness**: preserve build dependency metadata in generated archives for downstream tooling and inspection.
- **Docs sync**: update dependency and build documentation to reflect the archived build-dependency metadata behavior.

## [1.7.9] - 2026-03-18

### Changes
- **Build dependency expansion rework**: restructure `wbuild`'s dependency expansion flow for clearer behavior and maintenance.
- **Release sync**: roll forward package metadata and changelog for the latest builder changes.

## [1.7.8] - 2026-03-18

### Changes
- **CLI and docs clarification**: update command help and documentation to better explain dependency behavior and tool boundaries.
- **Build workflow wording cleanup**: align `wbuild` scope descriptions across CLI help and guides.

## [1.7.7] - 2026-03-18

### Changes
- **`wbuild` dependency scope controls**: refine dependency expansion and scope behavior for `wbuild run` and `wbuild deps`.
- **CLI documentation sync**: update usage and dependency reference docs to match the latest build-scope behavior.

## [1.7.6] - 2026-03-18

### Changes
- **Repository source configuration**: refine source handling and related defaults across repository resolution paths.
- **Docs and config sync**: update configuration and repository docs to match the latest source configuration behavior.

## [1.7.5] - 2026-03-18

### Changes
- **Installed DB migration tooling**: add a migration helper for installed database transitions.
- **Builder dependency cleanup**: refine staged `wbuild` dependency handling around the installed DB migration work.

## [1.7.4] - 2026-03-18

### Changes
- **`wbuild deps` summary improvements**: plan dependency output now reports repeated nodes separately from true dependency cycles.
- **Dependency graph tuning**: staged `wbuild` dependency expansion behavior was refined for clearer build-graph handling.

## [1.7.3] - 2026-03-18

### Changes
- **Dependency inspection clarity**: `wbuild deps` now clearly identifies plan-manifest output, while `wright deps` clearly identifies installed-database output.
- **CLI surface refactor**: command help and output were tightened across `wright`, `wbuild`, and `wrepo` to better match each tool's scope.
- **Documentation refresh**: CLI and workflow documentation were updated to reflect the current dependency, repository, and tool-boundary model.

## [1.7.2] - 2026-03-15

### Changes
- **Repository metadata moved to SQLite**: `wrepo` now stores repository metadata in `/var/lib/wright/repo/repo.db` by default, and creates the directory automatically if it does not exist.
- **Resolvers now prefer `wbuild` package metadata**: package resolution reads `.PARTINFO` metadata produced by `wbuild` and registered in the repo database, rather than treating the plan tree as a repository source.
- **`wbuild` now auto-registers built archives**: newly created `.wright.tar.zst` packages are added to the repo database immediately after a successful build; `wrepo sync` remains available for importing pre-existing archives.

### Documentation
- Update repository and CLI docs to describe the SQLite-backed repository catalog and binary-only `wrepo source` workflow.

## [1.7.1] - 2026-03-15

### Features
- **`--version` flag**: `wright`, `wbuild`, and `wrepo` now support `--version` / `-V` to display the version from Cargo.toml.
- **Git fetch progress bar**: replace multi-line log output during `git+` source fetches with a single-line indicatif progress bar, matching the HTTP download experience.

## [1.7.0] - 2026-03-15

### Breaking Changes
- **New `wrepo` binary**: repository management is now handled by a dedicated `wrepo` tool. The following commands have been removed from `wright` and `wbuild`:
  - `wright repo sync/list/remove` → `wrepo sync/list/remove`
  - `wright source add/remove/list` → `wrepo source add/remove/list`
  - `wright sync` → removed (`wrepo sync` prints stats after indexing)
  - `wright search --available` → `wrepo search` (`wright search` now only searches installed packages)
  - `wbuild index` → `wrepo sync` (eliminates duplication)

### Features
- **Three-tool architecture**: each binary now has a single clear responsibility:
  - `wbuild` — package constructor (plan.toml → .wright.tar.zst)
  - `wrepo` — repository manager (indexing, searching, source configuration)
  - `wright` — system administrator (install, remove, upgrade, query)
- **`wrepo sync` defaults to `components_dir`**: no directory argument needed for the common case (`wrepo sync` indexes `/var/lib/wright/components` by default)

### Documentation
- Rewrite usage.md with tool-by-tool structure, coordination workflows, and boundary summary table
- Rewrite architecture.md with three-binary diagram, module ownership matrix, and cross-tool coordination section
- Update getting-started.md with three-binary install instructions and `wrepo sync` workflow
- Update cli-reference.md with `wrepo` as a top-level section
- Update repositories.md for `wrepo` commands throughout

## [1.6.1] - 2026-03-15

### Features
- **Local repository management**: add `wright repo sync/list/remove` commands. `wright repo sync <dir>` generates the repository index directly from wright, replacing the need for `wbuild index` in the typical workflow. `wright repo list [name]` shows all available versions with `[installed]` markers. `wright repo remove` removes index entries with optional `--purge` to delete archive files.
- **Name-based upgrades**: `wright upgrade` now accepts package names in addition to file paths. The resolver finds the latest version from all configured sources. Use `--version` to target a specific version (enables downgrades).
- **Multi-version resolver**: `resolve_all()` returns all available versions of a part across sources, with `pick_latest()` and `pick_version()` helpers.

### Fixes
- **install_reason tracking for wbuild**: `wbuild run -i` now correctly marks user-specified targets as `explicit` and auto-resolved dependencies as `dependency`. Previously all packages were marked `explicit`.
- **Upgrade preserves install_reason**: upgrading a package via `wright upgrade` or `wbuild run -icf` no longer changes its install reason. Only `wright install` promotes a dependency to explicit — this is the only command that expresses intent to "own" a package.
- **sysupgrade version comparison**: fix naive version comparison to use proper epoch → version → release ordering, and pick the latest from all available versions instead of the first match.

### Documentation
- Update cli-reference, repositories, usage, and design-spec docs to reflect new commands and install_reason semantics.
- Sync database schema documentation in design-spec with actual schema (add epoch, assumed, install_scripts, install_reason columns; add optional_dependencies, provides, conflicts, shadowed_files tables).

## [1.6.0] - 2026-03-12

### Breaking Changes
- Remove support for legacy plan schemas. Plans now use top-level package metadata plus `[hooks]`, `[output]`, and `[output.<name>]`; output metadata is no longer embedded in `[lifecycle.fabricate]`.

### Features
- Add optional sibling `mvp.toml` support as a restricted MVP overlay for `plan.toml`. The overlay only accepts MVP dependency and lifecycle override fields.

### Changes
- Simplify plan manifests by moving install/remove hooks out of `lifecycle` and keeping `[lifecycle.fabricate]` as the final build-stage script only.
- Update fixtures, tests, and documentation to match the new manifest layout and recommended `plan.toml` + `mvp.toml` naming.

## [1.5.4] - 2026-03-11

### Changes
- Remove the unused `strip` option from plan and global config parsing, and reject stale keys instead of silently accepting them.
- Drop compatibility-only support for legacy package metadata and hook formats; packages now use `.PARTINFO` and `.HOOKS` only.
- Remove old `PKG_*` build variable aliases and rely on the current `PART_*` / `WRIGHT_BUILD_PHASE` interface.

## [1.5.3] - 2026-03-08

### Performance
- **Streaming output capture**: build output is now streamed to temp files instead of accumulated in memory. Log files are assembled via `io::copy`. Reduces peak memory usage and pipe backpressure during large compilations.

## [1.5.2] - 2026-03-08

### Features
- **Incremental builds**: the source tree (`src/`) is now preserved across builds when the build key is unchanged, skipping fetch/verify/extract. Plans that support incremental compilation (e.g. `make` without `make clean`) benefit from significantly faster rebuilds. Use `--clean` to force a full re-extraction.

### Changes
- Clarify the `compile: serialized` log message to `compile: one-at-a-time across dockyards` to avoid implying single-threaded compilation.

## [1.5.1] - 2026-03-08

### Changes
- **Rename "container" to "kit"**: package groupings for `wright install @name` are now called "kits" to avoid confusion with Docker/OCI containers. Config field `containers_dir` → `kits_dir`, TOML syntax `[[container]]` → `[[kit]]`, default path `/var/lib/wright/containers/` → `/var/lib/wright/kits/`.
- **Rename `hold_dir` to `plan_dir`** in builder internals for clarity.
- **Rename the plan output schema to fabricate**: plans now use `[lifecycle.fabricate]` and `[lifecycle.fabricate.<name>]` for final output metadata and split outputs, with `fabricate` also serving as the final lifecycle phase.
- **Builds no longer default to `/tmp`**: the default `build_dir` is now `/var/tmp/wright-build`, and dockyard overlay/root scratch directories live under the active build root instead of hardcoded `/tmp`.

### Fixes
- Dockyard temporary directories are cleaned up from the build root after each build, preventing stale accumulation in global `/tmp`.

## [1.4.1] - 2026-03-07

### Features
- Built-in `.zip` archive extraction using the pure-Rust `zip` crate — no external `unzip` tool required. Includes path traversal protection and Unix permission preservation.

## [1.4.0] - 2026-03-06

### Features
- **Kits**: new package grouping concept for `wright install @name`. Kits group packages (distinct from assemblies which group plans). One file per kit in `kits_dir`, filename is the kit name.
- **Repository management**: `wright source add/remove/list` commands to manage `repos.toml` without manual editing.
- **Repository indexing**: `wbuild index [PATH]` generates `wright.index.toml` from built packages for fast name-based resolution. Resolver uses the index when available, falls back to archive scanning.
- **Repository sync**: `wright sync` reports available packages from all indexed sources.
- **Available package search**: `wright search --available` (`-a`) searches indexed repos, showing `[installed]` tags for packages already on the system.
- **Flexible install targets**: `wright install` now accepts `.wright.tar.zst` file paths, package names (resolved from sources), and `@kit` references.
- **Skip installed packages**: `wbuild run -i` automatically skips packages already installed on the system. Use `--force` to rebuild anyway.
- New `docs/repositories.md` guide covering local repo creation, source management, indexing, and workflows.

### Changes
- **Assembly format**: one file per assembly (filename = assembly name), consistent with kits. Removed unused `AssembliesConfig::load()` single-file method.
- **Assemblies dir** default moved from `/etc/wright/assemblies` to `/var/lib/wright/assemblies` (data, not config).
- Assemblies and kits are documented as **non-dependent, combinatory groupings** — membership implies no dependency relationship. Multiple groups combine freely with deduplication.

## [1.3.1] - 2026-03-04

### Features
- Restructure `[sources]` from positional `uris`/`sha256` arrays to `[[sources]]` array-of-tables. Each source entry is now self-contained with `uri` and `sha256` fields.
- New `[relations]` section for `replaces`, `conflicts`, and `provides` — moved out of `[dependencies]` where they did not belong.
- Add `epoch` field to `[plan]` metadata (default 0). Epoch overrides version comparison for version scheme changes. Included in archive filename only when non-zero.
- Add `pre_install` hook — executed before file extraction during install and upgrade.
- Document `git+` URI format for cloning git repositories as sources (`git+https://...#tag`).
- Backward compatibility: old `[sources]`/`[dependencies]` relations syntax auto-converts with deprecation warnings. Mixing old and new syntax in the same file is rejected with a clear error.
- Migration script (`scripts/migrate-plans.py`) for batch-converting plan.toml files to the new schema.

## [1.3.0] - 2026-03-04

### Breaking Changes
- `[lifecycle.package]` (file install stage) renamed to `[lifecycle.staging]`. The default pipeline is now: `fetch → verify → extract → prepare → configure → compile → check → staging → fabricate`.
- `[split.<name>]` replaced by `[lifecycle.package.<name>]` (multi-package mode). All sub-packages including the main package must be explicitly declared.
- `[install_scripts]` and `[backup]` top-level sections replaced by `[lifecycle.package]` with `hooks.*` fields and `backup = [...]` (single-package mode), or per-sub-package fields in multi-package mode.
- `post_package` lifecycle stage removed.
- Package hook metadata file changed from `.INSTALL` (ini) to `.HOOKS` (TOML) inside `.wright.tar.zst` archives.

### Features
- New `[lifecycle.package]` section for single-package output declarations (hooks + backup).
- New `[lifecycle.package.<name>]` syntax for multi-package output, replacing `[split]`. Sub-packages support `description`, `version`, `release`, `arch`, `license`, `dependencies`, `script`, `hooks`, and `backup` fields.
- Single-package and multi-package modes are mutually exclusive with a clear error on conflict.
- Add `post_remove` hook support — executed after file removal during `wright remove`.
- Structured `.HOOKS` TOML format for hook storage in archives and database, replacing the `.INSTALL` ini format.
- Backward compatibility: old `[split]`/`[install_scripts]`/`[backup]` syntax auto-converts with deprecation warnings. Old `.INSTALL` ini in archives and database is transparently parsed via fallback.

## [1.2.8] - 2026-03-03

### Features
- Track install reason (`explicit` vs `dependency`) for each package. User-specified packages are marked `explicit`; dependencies resolved automatically during `wright install` are marked `dependency`. Existing packages default to `explicit` after migration.
- Add `wright remove --cascade` (`-c`) to automatically remove orphan dependencies — auto-installed packages that are no longer needed by any other installed package. Orphans are removed in leaf-first order.
- Add `wright list --orphans` (`-o`) to show auto-installed dependencies that no longer have any installed dependents.
- Explicitly installing a package that was previously pulled in as a dependency promotes it to `explicit`, protecting it from cascade removal.

## [1.2.7] - 2026-03-02

### Features
- Add `wright assume <name> <version>` to register externally-provided packages so dependency checks treat them as satisfied. Intended for bootstrapping scenarios where core packages (glibc, gcc, etc.) already exist but were not installed through wright. Assumed packages display with an `[external]` tag in `wright list` and are automatically replaced when a real package is installed via `wright install`.
- Add `wright unassume <name>` to remove an assumed package record.
- Add `wright list --assumed` (`-a`) to filter the package list to assumed packages only.
- Compile-stage serialization: when multiple dockyards run in parallel, compile stages are now serialized behind a semaphore so only one dockyard compiles at a time with access to all CPU cores. Non-compile stages (configure, package, etc.) remain fully parallel. This eliminates the "long-tail effect" where light packages finish quickly and leave cores idle while heavy compiles continue with a fraction of available cores.

### Fixes
- Dockyard `pivot_root` now falls back to `chroot` when running inside a chroot environment (e.g. LFS-style builds) where `pivot_root(2)` returns `EINVAL` due to the current root not being a real mount point.
- Lifecycle stage log messages now include the package name (e.g. `python: running stage: compile`) so concurrent dockyard output can be attributed to the correct package.
- Remove dead code: unused `compress_zstd`, `decompress_zstd`, and `sha256_bytes` functions.

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
- Archive support: `.tar.gz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.zip`
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

# Changelog

## [2.7.0] - 2026-04-20

### Added
- **Strongly-Typed Source Protocol**: Replaced untyped `[[sources]]` URIs with a structured, protocol-based format.
    - New `type = "http"`, `type = "git"`, and `type = "local"` variants provide per-protocol parameter validation.
    - Explicit `url`, `path`, `ref`, and `depth` fields replace the previous `git+https://...#ref` string parsing.
    - Automated migration script provided in `tools/migrate_sources.py`.


## [2.6.1] - 2026-04-20

### Added
- **Shallow Git Support**: Added `depth` field to `[[sources]]` for Git URIs, enabling faster downloads for large repositories.

### Changed
- **Git Performance Optimization**: 
    - Switched local repository clones to use **hardlinks** (shared object databases). This reduces extraction time and disk usage for large Git sources (like LLVM) by several gigabytes and minutes.
    - Improved fetch feedback with a dedicated \"indexing\" status to prevent appearing stuck during long post-download processing.

## [2.6.0] - 2026-04-20

### Changed
- **Physical Workspace Refactoring**: Renamed the default build root from `/var/tmp/wright-build` to **`/var/tmp/wright/workshop`** for a more thematic "craftsman" feel.
- **Directory Semantics**: Internal build directories have been renamed for better clarity:
    - `src/` $\rightarrow$ **`work/`** (host-side storage for `${WORKDIR}`)
    - `pkg/` $\rightarrow$ **`output/`** (host-side storage for `${PART_DIR}`)
    - `log/` $\rightarrow$ **`logs/`** (standardized plural naming)
- **Architecture**: Internal Rust structs (e.g. `BuildResult`) have been updated to align with the new naming scheme.

### Added
- **Source Autoflattening**: Enhanced `extract_to` with an automatic "flattening" logic. If an archive contains only a single top-level directory (like most GitHub tarballs), its contents are automatically moved up to the root of the specified `extract_to` directory. This ensures that `cd source` (or similar) is always deterministic regardless of the archive's internal folder name.

## [2.5.1] - 2026-04-20

### Changed
- **Removed `${BUILD_DIR}` (Breaking)**: The magic `${BUILD_DIR}` variable and its non-deterministic directory detection logic have been completely removed.
- **Explicit Navigation**: Scripts now always start at the root of `${WORKDIR}`. Developers must explicitly `cd` into subdirectories if needed.

### Added
- **Deterministic Sources**: Added `as` and `extract_to` fields to `[[sources]]`. 
    - `as`: Rename the downloaded file in the local cache.
    - `extract_to`: Force extraction/copying of a source into a specific subdirectory under `${WORKDIR}`, enabling 100% predictable paths in scripts.

## [2.5.0] - 2026-04-20

### Changed
- **Path Variable Refactoring (Breaking)**: Renamed `${SRC_DIR}` to **`${WORKDIR}`** throughout the system.
- **Simplified Directory Structure**: Merged `${FILES_DIR}` into **`${WORKDIR}`**. Non-archive source files are now copied directly into the root of the work directory.
- **Improved Scripting Experience**: Developers now use `${WORKDIR}` as the primary reference for all input files, reducing cognitive load.
- **Robust Path Matching**: Introduced path canonicalization in the resolver to ensure reliable target identification when using symlinks or relative paths.

### Fixed
- **Compilation Error**: Fixed a regression in `PlanManifest::validate` where the `isolation` field was incorrectly accessed on single-output plans.
- **Linter Accuracy**: Corrected the build set filtering logic to prevent explicit targets from being erroneously skipped during `apply --force`.

## [2.4.1] - 2026-04-20

### Added
- **`wright lint` command**: Introduced a dedicated subcommand for static validation of plan files. It performs deep structural and semantic checks (isolation levels, version formats, etc.) and reports clear errors with file paths.

### Changed
- **Strengthened Plan Validation**: Integrated mandatory `validate()` into the plan loading pipeline. Parsing errors are now immediately fatal, preventing invalid plans from being silently skipped or causing downstream failures.
- **Improved Path Robustness**: Resolution now uses path canonicalization to ensure consistent matching when plans are referenced via symlinks or relative paths.
- **Enhanced Error Messages**: Replaced generic "converged" messages with more actionable feedback when build targets are filtered.

## [2.4.0] - 2026-04-20

### Changed
- **Terminology**: Renamed the namespace-based execution environment from `dockyard` to `isolation` throughout the codebase, documentation, and configuration.
- **Configuration**: Updated the `plan.toml` schema to use `isolation = "..."` instead of `dockyard`.
- **Validation**: Hardened isolation level parsing. Invalid values for the `isolation` field (e.g. `relax` instead of `relaxed`) now cause an immediate build error instead of silently falling back to `strict`.

## [2.3.5] - 2026-04-20

### Changed
- **Build logging**: Stage log files (e.g. `compile.log`) are now written in real time during execution. The log file is created before the stage starts and subprocess stdout is streamed into it immediately, making `tail -f <work_dir>/<name>-<version>/log/compile.log` usable while a build is in progress. Stderr and the exit code/duration footer are appended after the stage completes.

## [2.3.4] - 2026-04-19

### Changed
- **Architecture**: Split the monolithic `system.rs` command handler into a modular `system/` directory with dedicated submodules (`apply`, `install`, `list`, `doctor`).
- **Configuration**: Migrated to `figment` for configuration parsing, enabling robust multi-layer TOML merging and support for environment variable overrides (`WRIGHT_*`).
- **Transactions**: Redesigned the rollback journal format to use robust JSON Lines (`serde_json`), eliminating fragile manual escaping logic and improving resilience against special characters in filenames.
- **Transactions**: Extracted DAG (Directed Acyclic Graph) dependency sorting into an isolated `transaction::dag` module.
- **Testing**: Introduced the `TestPartBuilder` builder pattern for generating unit test fixtures, significantly improving test readability and maintainability.


## [2.3.3] - 2026-04-17

### Fixed
- **Local Dependency Resolution**: Fixed an issue where `wright apply` failed to resolve dependencies that were located in the current working directory. The local resolver now consistently includes the current directory in its search paths for both plans and parts.

## [2.3.2] - 2026-04-17

### Fixed
- **ETXTBSY on cross-device installs**: Added an explicit `unlink` before `copy` when falling back to cross-device file installation. This prevents "Text file busy" errors when updating a running executable (like `bash`) where the staging and target directories are on different filesystems.

## [2.3.1] - 2026-04-15

### Changed
- **Terminology & Naming**: Renamed the `inventory` concept and directory to `archive` (e.g., `inventory_db_path` config was renamed to `archive_db_path`, and `InventoryDb` to `ArchiveDb`).
- **Database Optimization**: Cleaned up the `ArchiveDb` schema by removing unused `INSERT` statements for `provides`, `conflicts`, and `replaces` to improve execution performance. The tables remain in the schema for backward compatibility and future expansion.
- **Documentation**: Added comprehensive `docs/database.md` to explain the dual-database architecture (`installed.db` vs `archives.db`).
- **Chores**: Resolved minor Clippy warnings (`module_inception` and `needless_borrows_for_generic_args`).

## [2.3.0] - 2026-04-15

### Changed
- **Removed build-result cache**: eliminated `cache_dir/builds/` — the compressed per-part artifact cache that stored `pkg/` and `log/` snapshots keyed by build hash. Parts are already persisted as `.wright.tar.zst` files and installed into the database; the intermediate cache served no additional purpose.
- **`source_dir` replaces `cache_dir`**: the `cache_dir` config field is renamed to `source_dir` (default `/var/lib/wright/sources`), removing the now-redundant `cache/sources/` nesting.
- **Dedicated staging directory**: install and upgrade transactions now use `/var/lib/wright/staging/` for temporary extraction, replacing the previous behaviour of deriving the staging path from the part file location (which polluted `/var/lib/wright/` with `wright-stage-*` directories on crash).

## [2.2.0] - 2026-04-14

### Changed
- **`apply` default convergence policy**: `wright apply` now defaults to `--match=outdated`, so plan-driven runs naturally cover first installs, upgrades, and missing/outdated upstream dependencies in one command.
- **`apply` no-op behavior**: when requested targets already match the current plan state under the selected policy, `wright apply` now exits successfully instead of surfacing a confusing empty-build error.
- **CLI/docs alignment**: `--match` is now the primary flag name for resolve/apply match policies, with documentation updated to present `apply` as Wright's natural plan-driven install/upgrade/dependency combo workflow.
- **Plan metadata variable cleanup**: removed the legacy `${PART_NAME}` / `${PART_VERSION}` / `${PART_RELEASE}` / `${PART_ARCH}` aliases in favor of `${NAME}` / `${VERSION}` / `${RELEASE}` / `${ARCH}`.
- **Plan authoring docs refresh**: updated plan-writing docs, examples, and fixtures to prefer `${BUILD_DIR}` plus the shorter metadata variable names.
- **System plan tree migration**: migrated the maintained `/var/lib/wright/plans` tree away from the old `PART_*` metadata variables.

### Fixed
- **Split-output variable context**: `${MAIN_PART_NAME}` and `${MAIN_PART_DIR}` are now documented and consistently available for split-output fabricate scripts.

## [2.1.9] - 2026-04-13

### Fixed
- **`apply --force` target preservation**: `wright apply --force` now keeps explicitly requested installed targets in the apply build set, so force-rebuild runs no longer collapse into `No targets specified to build.` under the default `missing` match policy.

## [2.1.8] - 2026-04-13

### Changed
- **`apply` smart defaults restored**: `wright apply` now expands missing upstream dependency plans by default, so source-first maintenance runs can converge without an extra manual resolve step.
- **Apply docs refresh**: updated the apply design notes, usage guide, assemblies docs, and related references to match the current default dependency-expansion behavior.
- **CLI/docs cleanup**: removed stale references to legacy `--deps=sync`/`--print-archives` syntax and aligned command examples with the current `--match` and `--print-parts` interfaces.

## [2.1.7] - 2026-04-13

### Changed
- **State-path rename**: renamed the default installed-state and archive-inventory databases to `/var/lib/wright/state/installed.db` and `/var/lib/wright/state/archives.db`, keeping lock files under `/var/lib/wright/lock/`.
- **Part-store naming cleanup**: renamed the user-facing `components_dir` configuration to `parts_dir` and updated command code, examples, and documentation accordingly.
- **Docs refresh**: added assembly-writing links to the main docs entry points and refreshed related architecture, usage, logging, and troubleshooting references.

## [2.1.6] - 2026-04-13

### Changed
- **Compact diversion logging**: Warning logs for file diversion during `install` and `upgrade` are now aligned with the overall logging style and automatically abbreviate long file paths to improve readability.

## [2.1.5] - 2026-04-13

### Added
- **File diversion**: Wright now automatically resolves file conflicts during `install` and `upgrade` by diverting the original files to `.wright-diverted` rather than aborting. Diverted files are safely restored when the shadowing part is removed.

### Changed
- **Scoped fetch logging**: Source fetch logs are now consistently scoped to their respective plans (e.g., `[gdb] Fetched ...`) for clearer progress output during concurrent builds.

## [2.1.4] - 2026-04-12

### Changed
- **Removed user-configurable build concurrency**: Wright no longer supports `[build].isolations` or `wright build --isolations`; build task parallelism is now selected automatically from the usable CPU budget.
- **Removed global compiler-flag config**: Wright no longer supports `[build].cflags` or `[build].cxxflags`; compiler flags should now be set per plan, per stage, or via the invoking environment.

## [2.1.3] - 2026-04-12

### Changed
- **`apply --force` semantics**: `wright apply --force` now performs a clean rebuild across the apply pipeline by clearing each plan's build workspace and build cache before rebuilding, while still reusing the source download cache.

## [2.1.2] - 2026-04-12

### Changed
- **CLI Refactor**: Renamed `apply --force-build` to `apply --force` (`-f`) and consolidated the `force` behavior to trigger both a forced rebuild and a forced re-installation.

## [2.1.1] - 2026-04-12

### Changed
- **CLI/API Refactor**: Finalized API refinement for `apply` and `resolve` commands:
  - Replaced ambiguous rebuild/filter flags with a explicit `--match` flag (supporting `missing`, `outdated`, `installed`, `all`) to provide total control over installation state selection.
  - Standardized reverse dependency flag to `--rdeps`.
  - Removed all implicit default behaviors in `apply` to ensure strict, explicit user control.

## [2.1.0] - 2026-04-12

### Fixed
- **Pipe-safe verbose build output**: `wright build --print-parts` now keeps stdout reserved for part paths even under `-v`, while live subprocess output is mirrored to stderr so `... | wright install` pipelines remain observable and machine-safe.

## [2.0.3] - 2026-04-11

### Changed
- **Build CLI/doc alignment**: cleaned up the unified `wright` build command layout across the codebase and refreshed the associated documentation and examples.
- **`resolve` flag rename**: renamed `--self` to `--include-targets` (`-s`) so the option more clearly describes including the listed targets in resolve output.

### Added
- **`apply` design notes**: added `docs/apply-design.md` to document how `wright apply` chooses build scope and keeps rebuild pressure separate from install behavior.

## [2.0.2] - 2026-04-11

### Changed
- **Flattened CLI layout**: lifted `resolve`, `check`, `fetch`, `checksum`, and `prune` out of nested command groups, and converted build-only validation/fetch/checksum flows into `build` flags.
- **Unified lock API**: consolidated named-lock and DB-lock acquisition behind a single `acquire_lock` interface with explicit lock identity and mode types.

### Added
- **`apply --dry-run`**: `wright apply` now supports `--dry-run` / `-n` to preview the build and install plan without making changes.

### Fixed
- **`apply` planning and failure handling**: removed duplicate explicit target resolution, isolated `:bootstrap` suffix handling, delayed DB opening in the apply path, and report already-installed parts when a partial apply fails.

## [2.0.1] - 2026-04-11

### Changed
- **Merged `wbuild` into `wright`**: the standalone `wbuild` binary has been removed. All build, plan, and inventory management commands are now subcommands of the unified `wright` CLI tool.
- **Refactored CLI architecture**: adopted a Thin Binary + Command Dispatcher pattern. The `wright` entry point is now a lightweight wrapper that dispatches to encapsulated command handlers, improving maintainability and testability.
- **Unified man pages**: the build process now generates a single set of hierarchical man pages for the integrated `wright` tool.

### Fixed
- **Compiler warnings**: eliminated all unused imports and type-mapping warnings introduced during the CLI consolidation.

## [2.0.0] - 2026-04-11

### Changed
- **Inventory module replaces `wrepo`**: the `wrepo` binary and `src/repo/` module have been removed in favour of a new `src/inventory/` module that manages the local part database. The tool set is now two binaries: `wbuild` and `wright`.
- **Simplified `ResolveOptions`**: the `install` field has been removed from `ResolveOptions`; all dependency types are now always expanded, removing a source of inconsistent resolve behaviour.

### Fixed
- **`apply` self-deadlock**: `apply` now opens a fresh DB connection per batch instead of holding the main handle across `resolve_build_set`, which previously caused a deadlock on `parts.db.lock`.
- **Tracing message capitalisation**: `INFO`/`DEBUG` messages now consistently use sentence case throughout.

### Added
- **Stdin support for `apply` and `install`**: both commands now accept targets/parts piped from stdin, enabling workflows like `wbuild resolve neovim --self | wright apply`.

## [1.12.5] - 2026-04-09

### Fixed
- **Local source cache refresh**: local files (patches, config files) are now always re-copied to the source cache on each fetch instead of being skipped when the cache entry already exists. This ensures that adding or modifying a patch in the plan directory is picked up correctly after `--clean`.

## [1.12.4] - 2026-04-08

### Fixed
- **Download retries**: HTTP(S) downloads now retry up to 3 times on transient network errors (e.g. `request or response body error`), preventing spurious failures when fetching large packages like sqlite.

## [1.12.3] - 2026-04-08

### Performance
- **Remove performance**: `wright remove` now uses batch DB queries for file-owner checks instead of one query per file, and adds an index on `files(path)`. For large packages like texlive-texmf (~300k files), this reduces removal time from minutes to seconds.

## [1.12.2] - 2026-04-08

### Performance
- **Install conflict checks**: `wright install` and `wbuild run -i` now batch file-owner lookups during install instead of querying SQLite once per file, reducing install-time database overhead for very large parts.

### Changes
- **Fixed lock-file layout**: database locks now use stable files under `/var/lib/wright/lock/` (for example `parts.db.lock` and `repo.db.lock`) and rely on `flock(2)` lifetime instead of creating and deleting transient lock files beside the databases.
- **Install and upgrade diagnostics**: `-v` now emits debug timing for part extraction, file scanning, owner checks, filesystem writes, database updates, hooks, and total install/upgrade time.
- **Log wording cleanup**: install and upgrade `INFO` messages now use natural sentence order (for example `Installing texlive-texmf: 252553 files`) for easier reading.

## [1.12.1] - 2026-04-08

### Performance
- **Upgrade performance**: conflict check and old-file owner lookup during `wright upgrade` now use batch DB queries (up to 999 paths per round-trip) instead of one query per file, reducing ~33000 SQLite operations to ~34 for large packages like go (16681 files).

## [1.12.0] - 2026-04-08

### Performance
- **Install performance**: part SHA-256 is now computed in a single streaming pass during extraction, eliminating a full re-read of the part after install.
- **Same-filesystem staging**: staging temp dir is placed under the wright data dir (same filesystem as `/`) so `rename(2)` replaces read+write copy during file installation; falls back to copy on cross-device installs.
- **Parallel file operations**: `collect_file_entries` and `copy_files_to_root` now process files in parallel with rayon while preserving serial directory creation and rollback safety.

## [1.11.8] - 2026-04-08

### Fixed
- **Symlink install over directory**: fix install failure when a part's symlink destination already exists as a real directory on the host (`Is a directory` error).

## [1.11.7] - 2026-04-07

### Fixed
- **Isolation DNS resolution**: Fix dangling `/etc/resolv.conf` symlink issue in isolation by ensuring essential `/etc` files are properly bind-mounted after `/run` is mounted, supporting systems using `systemd-resolved` with `xray-tproxy`.

## [1.11.6] - 2026-04-07

### Changes
- **Simplified `-i` install flow**: `wbuild run -i` now installs completed packages directly to host `/` between build waves, matching the behavior of `wright install`. Removes the session-local overlay sysroot and staged package database introduced in 1.11.4–1.11.5. This eliminates the `isolation = "none"` incompatibility and makes `-i` work consistently across all isolation levels.

## [1.11.5] - 2026-04-04

### Changes
- **Session-root native install stabilization**: `wbuild run -i` now stages native isolation builds against the session root and commits them to the host only after the build graph succeeds, avoiding live-root mutation during parallel construction.
- **Install hook timing cleanup**: package install and upgrade hooks are deferred until the final host-root commit instead of running inside the staging sysroot.
- **Scheduling summary clarity**: `wbuild run` now reports dependency-wave batches (`Scheduling batch N ...`) rather than misleading topological depth values.
- **Regression coverage and docs sync**: add isolation shebang execution regression tests and update the architecture, CLI, cookbook, usage, plan-writing, and design docs to match the current session-root behavior.

## [1.11.4] - 2026-04-03

### Changes
- **Wave-based scheduling summaries**: `wbuild run` now prints dependency-wave batches (`Scheduling batch N ...`) instead of topological-depth annotations, avoiding confusion with `wbuild resolve --depth`.
- **Session sysroot installs**: `wbuild run -i` now builds against a session-local overlay root and staged package database, then commits all successful outputs to the host root at the end. This prevents build sandboxes from observing a live host root that is being mutated by concurrent auto-installation.
- **Deferred install hooks during staging**: when `wbuild run -i` stages packages into the session root, package install/upgrade hooks are deferred until the final host-root commit instead of running inside the staging sysroot.

## [1.11.3] - 2026-03-25

### Changes
- **Pipe-friendly `wright list`**: print only part names by default for easy piping; add `-l`/`--long` flag to show origin, version, release, and arch.
- **`wright mark` command**: change a part's install origin between `manual` and `dependency`, enabling flexible orphan cleanup workflows.

## [1.11.2] - 2026-03-23

### Changes
- **Unified source fetch UI**: align HTTP, `git+`, and local-file source fetch progress under one shared presentation model with consistent prefixes, progress styles, and completion messages.
- **Progress cleanup**: clear completed source-transfer progress bars before later lifecycle stages so stale network-fetch lines do not remain visible under `compile`.

## [1.11.1] - 2026-03-23

### Changes
- **Core module refactor**: split oversized transaction, builder orchestrator, manifest, and database modules into focused submodules while preserving the existing public API.
- **Validation coverage**: run the full library and integration test suites after the refactor to confirm build, install, query, and verification flows still behave correctly.
- **Design spec sync**: update the technical design document to describe the current source module layout.

## [1.11.0] - 2026-03-21

### Changes
- **`wbuild resolve` pipeline**: dependency expansion (`--deps`, `--deps=sync`, `--deps=all`) is now performed by `wbuild resolve` and piped into `wbuild run`, replacing the previous `wbuild run --deps` interface. This enables `wbuild resolve ... | sudo wbuild run ...` workflows where resolution runs unprivileged.
- **Unified database path**: non-root users now default to the system database (`/var/lib/wright/db/parts.db`) so that resolve and build pipelines across privilege boundaries consult the same installation state. Per-user overrides via config are still supported.
- **Multi-isolation progress spinners**: parallel builds now display per-isolation progress spinners showing the current lifecycle stage, replacing interleaved log lines.
- **Wave-based transitive rebuild expansion**: fix `expand_rebuild_deps` to collect each depth level into a separate wave, preventing packages added at the current depth from triggering further expansion within the same iteration.

## [1.10.1] - 2026-03-21

### Changes
- **Database lock retry**: replace immediate-fail `flock` with retry loop (exponential backoff from 50ms to 1s, 30s timeout) so parallel build tasks queue for the database lock instead of failing.
- **Documentation refresh**: update Origin terminology (`explicit`→`manual`, add `build` tier), document `--skip-check` and `--clear-sessions` flags, fix stale commands and config fields in design-spec.

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
- **part metadata completeness**: preserve build dependency metadata in generated parts for downstream tooling and inspection.
- **Docs sync**: update dependency and build documentation to reflect the partd build-dependency metadata behavior.

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
- **`wbuild` now auto-registers built parts**: newly created `.wright.tar.zst` packages are added to the repo database immediately after a successful build; `wrepo sync` remains available for importing pre-existing parts.

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
- **`wrepo sync` defaults to `parts_dir`**: no directory argument needed for the common case (`wrepo sync` indexes `/var/lib/wright/parts` by default)

### Documentation
- Rewrite usage.md with tool-by-tool structure, coordination workflows, and boundary summary table
- Rewrite architecture.md with three-binary diagram, module ownership matrix, and cross-tool coordination section
- Update getting-started.md with three-binary install instructions and `wrepo sync` workflow
- Update cli-reference.md with `wrepo` as a top-level section
- Update repositories.md for `wrepo` commands throughout

## [1.6.1] - 2026-03-15

### Features
- **Local repository management**: add `wright repo sync/list/remove` commands. `wright repo sync <dir>` generates the repository index directly from wright, replacing the need for `wbuild index` in the typical workflow. `wright repo list [name]` shows all available versions with `[installed]` markers. `wright repo remove` removes index entries with optional `--purge` to delete part files.
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
- Clarify the `compile: serialized` log message to `compile: one-at-a-time across isolations` to avoid implying single-threaded compilation.

## [1.5.1] - 2026-03-08

### Changes
- **Rename "container" to "kit"**: package groupings for `wright install @name` are now called "kits" to avoid confusion with Docker/OCI containers. Config field `containers_dir` → `kits_dir`, TOML syntax `[[container]]` → `[[kit]]`, default path `/var/lib/wright/containers/` → `/var/lib/wright/kits/`.
- **Rename `hold_dir` to `plan_dir`** in builder internals for clarity.
- **Rename the plan output schema to fabricate**: plans now use `[lifecycle.fabricate]` and `[lifecycle.fabricate.<name>]` for final output metadata and split outputs, with `fabricate` also serving as the final lifecycle phase.
- **Builds no longer default to `/tmp`**: the default `build_dir` is now `/var/tmp/wright-build`, and isolation overlay/root scratch directories live under the active build root instead of hardcoded `/tmp`.

### Fixes
- Isolation temporary directories are cleaned up from the build root after each build, preventing stale accumulation in global `/tmp`.

## [1.4.1] - 2026-03-07

### Features
- Built-in `.zip` part extraction using the pure-Rust `zip` crate — no external `unzip` tool required. Includes path traversal protection and Unix permission preservation.

## [1.4.0] - 2026-03-06

### Features
- **Kits**: new package grouping concept for `wright install @name`. Kits group packages (distinct from assemblies which group plans). One file per kit in `kits_dir`, filename is the kit name.
- **Repository management**: `wright source add/remove/list` commands to manage `repos.toml` without manual editing.
- **Repository indexing**: `wbuild index [PATH]` generates `wright.index.toml` from built packages for fast name-based resolution. Resolver uses the index when available, falls back to part scanning.
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
- Add `epoch` field to `[plan]` metadata (default 0). Epoch overrides version comparison for version scheme changes. Included in part filename only when non-zero.
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
- Package hook metadata file changed from `.INSTALL` (ini) to `.HOOKS` (TOML) inside `.wright.tar.zst` parts.

### Features
- New `[lifecycle.package]` section for single-package output declarations (hooks + backup).
- New `[lifecycle.package.<name>]` syntax for multi-package output, replacing `[split]`. Sub-packages support `description`, `version`, `release`, `arch`, `license`, `dependencies`, `script`, `hooks`, and `backup` fields.
- Single-package and multi-package modes are mutually exclusive with a clear error on conflict.
- Add `post_remove` hook support — executed after file removal during `wright remove`.
- Structured `.HOOKS` TOML format for hook storage in parts and database, replacing the `.INSTALL` ini format.
- Backward compatibility: old `[split]`/`[install_scripts]`/`[backup]` syntax auto-converts with deprecation warnings. Old `.INSTALL` ini in parts and database is transparently parsed via fallback.

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
- Compile-stage serialization: when multiple isolations run in parallel, compile stages are now serialized behind a semaphore so only one isolation compiles at a time with access to all CPU cores. Non-compile stages (configure, package, etc.) remain fully parallel. This eliminates the "long-tail effect" where light packages finish quickly and leave cores idle while heavy compiles continue with a fraction of available cores.

### Fixes
- Isolation `pivot_root` now falls back to `chroot` when running inside a chroot environment (e.g. LFS-style builds) where `pivot_root(2)` returns `EINVAL` due to the current root not being a real mount point.
- Lifecycle stage log messages now include the package name (e.g. `python: running stage: compile`) so concurrent isolation output can be attributed to the correct package.
- Remove dead code: unused `compress_zstd`, `decompress_zstd`, and `sha256_bytes` functions.

## [1.2.6] - 2026-02-22

### Features
- Add `wbuild run --skip-check` to skip only the lifecycle `check` stage while still running a full build pipeline (including fetch/verify/extract), without requiring `--stage` partial-build mode.

### Fixes
- Config files declared in `[backup]` now create `<path>.wnew` only when the live file already exists during upgrade. If the config path does not exist yet, the new file is installed directly to `<path>`.
- Isolation CPU budgets are now partitioned fairly across each launch wave, avoiding misleading same-wave allocations like `16`, `8`, `5` that summed above the host CPU count.

## [1.2.5] - 2026-02-22

### Changes
- CPU scheduling default now uses all available CPUs when `[build].max_cpus` is unset (instead of implicitly reserving 4 cores for the OS). The isolation status line no longer prints the "reserved 4 for OS" note.

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
- FHS validation after the `package` stage: every file and symlink in `$PKG_DIR` is checked against the distribution's merged-usr path whitelist before the part is created. Violations produce a `ValidationError` with a clear hint (e.g. "install to /usr/bin"). Absolute symlink targets are also validated. Set `[options] skip_fhs_check = true` to bypass for edge cases such as kernel modules.

## [1.2.2] - 2026-02-21

### Changes
- Remove `optional` field from lifecycle stages. Stages either run and must pass, or are skipped via `--stage`. Use `--stage` to omit the `check` stage instead of silently ignoring test failures.

## [1.2.1] - 2026-02-21

### Changes
- Replace `--until` and `--only` lifecycle flags with a unified `--stage` flag that accepts multiple values (e.g. `--stage check --stage package`). Empty `--stage` runs the full pipeline; one or more `--stage` values run exactly those stages in pipeline order, skipping fetch/verify/extract (requires a previous full build).
- `wbuild fetch` now correctly stops after source extraction without running lifecycle stages.

## [1.2.0] - 2026-02-21

### Features
- Rename "sandbox" isolation environment to "isolation" throughout codebase, config, TOML fields, and docs
- Rename "worker" concurrency concept to "isolation" for consistency (`workers` → `isolations`, `nproc_per_worker` → `nproc_per_isolation`)
- Add `max_cpus` config field to hard-cap total CPU cores used; defaults to `available_cpus - 4` (minimum 1) to reserve headroom for the OS
- Print resource info at build start: active isolation count, total/available CPUs, and per-isolation CPU share
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
- Fix spurious WARN on part root entry during package creation
- Track `Cargo.lock` for reproducible builds (removed from `.gitignore`)

## [1.0.0] - 2026-02-20

### Features
- Declarative TOML-based package plans with configure / compile / package lifecycle stages
- Sandboxed builds using Linux namespaces (mount, PID, network isolation)
- Split-package support for producing multiple output packages from a single build
- Bootstrap mode for building the initial system toolchain
- Git source support alongside HTTP part downloads
- Dependency resolution with build / runtime / link classification
- `replaces` and `conflicts` fields for package compatibility management
- `doctor` subcommand for system health diagnostics
- Stage-level exec (`wbuild run <pkg> --stage <stage>`) for targeted rebuilds
- Resource limits on build processes to prevent runaway builds
- SHA-256 checksum verification for downloaded sources
- part support: `.tar.gz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.zip`
- Symlink-aware tar packaging (parts symlinks as symlinks, not followed)
- Special file handling in parts (FIFO, char/block devices)
- Progress indicators for downloads and package operations
- Structured logging with `RUST_LOG` / `--log-level` control
- SQLite-backed package database

### Fixes
- Resolved empty root-entry warning during tar part creation
- Fixed unsafe part path detection (empty path vs. path traversal)
- Fixed URI name substitution for packages with version-templated URLs
- Fixed duplicate/conflicting file handling across split packages
- Mitigated potential resource exhaustion in allocation paths
- Correct `BUILD_DIR` remapping inside sandboxed environments

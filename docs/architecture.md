# Architecture

Single binary (`src/bin/wright.rs`) with subcommands, backed by a library crate (`src/lib.rs`).

## Module Map

```
src/
├── lib.rs                      # Library root
├── error.rs                    # Error types (thiserror)
├── config.rs                   # GlobalConfig — wright.toml loading
├── package/
│   ├── manifest.rs             # PackageManifest — plan.toml parsing + validation
│   ├── version.rs              # Version parsing and comparison (segment-based)
│   └── archive.rs              # Binary package creation (tar.zst with .PKGINFO/.FILELIST)
├── builder/
│   ├── mod.rs                  # Builder — build pipeline orchestrator
│   ├── lifecycle.rs            # Stage ordering and execution
│   ├── executor.rs             # Executor loading, script execution
│   └── variables.rs            # ${VAR} substitution engine
├── database/
│   ├── mod.rs                  # SQLite connection and queries
│   └── schema.rs               # Schema (packages, files, dependencies, transactions)
├── transaction/
│   ├── mod.rs                  # install, remove, upgrade, verify
│   └── rollback.rs             # Rollback on failure
├── resolver/
│   ├── graph.rs                # Dependency graph
│   └── topo.rs                 # Topological sort
├── sandbox/
│   └── bwrap.rs                # Bubblewrap command generation
├── repo/
│   ├── index.rs                # Repository index parsing
│   ├── sync.rs                 # Remote sync
│   └── source.rs               # Source resolution (priority-based)
└── util/
    ├── download.rs             # HTTP downloads
    ├── checksum.rs             # SHA-256
    └── compress.rs             # tar.zst compression/decompression
```

## Layers

```
┌───────────────────────────────────────────┐
│              CLI (wright)                  │
├───────────────────────────────────────────┤
│   Resolver  │  Transaction  │  Builder    │
├───────────────────────────────────────────┤
│  Database │ Sandbox │ Executor │ Utilities │
├───────────────────────────────────────────┤
│    filesystem, namespace, process, network │
└───────────────────────────────────────────┘
```

## Data Flow: Build

```
plan.toml → PackageManifest::from_file()
  → Builder::build() (create src/, pkg/, log/ dirs)
  → fetch + verify + extract sources
  → apply patches
  → LifecyclePipeline::run() (variable substitution → executor → sandbox → log)
  → archive::create_archive() → .wright.tar.zst
```

## Data Flow: Install

```
.wright.tar.zst → transaction::install_package()
  → extract to temp dir → parse .PKGINFO
  → BEGIN TRANSACTION → insert package + files + deps
  → copy files to root → COMMIT
  → on error: rollback (delete files, ROLLBACK)
```

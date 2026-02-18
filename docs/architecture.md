# Architecture

Single binary (`src/bin/wright.rs`) with subcommands, backed by a library crate (`src/lib.rs`).

## Module Map

```
src/
├── lib.rs                          # Library root
├── error.rs                        # Error types (thiserror)
├── config.rs                       # GlobalConfig — wright.toml loading
├── package/
│   ├── manifest.rs                 # PackageManifest — plan.toml parsing + validation
│   ├── version.rs                  # Version parsing and comparison (segment-based)
│   └── archive.rs                  # Binary package creation (tar.zst with .PKGINFO/.FILELIST)
├── builder/
│   ├── mod.rs                      # Builder — build pipeline for a single package
│   ├── orchestrator.rs             # Multi-target build scheduling, --rebuild-deps expansion
│   ├── lifecycle.rs                # Stage ordering and execution
│   ├── executor.rs                 # Executor loading, script execution
│   └── variables.rs                # ${VAR} substitution engine
├── database/
│   ├── mod.rs                      # SQLite connection, queries, recursive dep traversal
│   └── schema.rs                   # Schema (packages, files, dependencies, transactions)
├── transaction/
│   ├── mod.rs                      # install, remove (with dep protection), upgrade, verify
│   └── rollback.rs                 # Rollback on failure
├── query/
│   └── mod.rs                      # Dependency tree rendering (forward and reverse)
├── sandbox/
│   ├── mod.rs                      # Sandbox trait and dispatch
│   ├── bwrap.rs                    # Bubblewrap sandbox (namespace isolation)
│   └── native.rs                   # Native sandbox (direct execution)
├── repo/
│   ├── index.rs                    # Repository index parsing
│   ├── sync.rs                     # Remote sync
│   └── source.rs                   # Source resolution (priority-based)
└── util/
    ├── download.rs                 # HTTP downloads
    ├── checksum.rs                 # SHA-256
    └── compress.rs                 # tar.zst compression/decompression
```

## Layers

```
┌──────────────────────────────────────────────┐
│          CLI  (src/bin/wright.rs)             │
│  Pure dispatch — CLI definition + match       │
├──────────────────────────────────────────────┤
│  Orchestrator  │  Transaction  │    Query     │
│  (parallel     │  (dep-safe    │  (dep tree,  │
│   builds,      │   remove,     │   analysis)  │
│   rebuild-deps)│   rollback)   │              │
├──────────────────────────────────────────────┤
│  Builder │ Database │ Sandbox │ Executor │ Util│
├──────────────────────────────────────────────┤
│     filesystem, namespace, process, network   │
└──────────────────────────────────────────────┘
```

## Data Flow: Build

```
plan.toml → PackageManifest::from_file()
  → orchestrator::run_build()
      → resolve targets → expand --rebuild-deps
      → build dependency map → topological sort
      → parallel execution (thread pool):
          → Builder::build() per package
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

## Data Flow: Remove

```
wright remove <pkg>
  → check dependents → refuse if non-empty (unless --force)
  → with --recursive: collect transitive dependents (leaf-first)
  → remove each in safe order → delete files → remove from DB

wright deps <pkg> [--reverse]
  → query::print_dep_tree / print_reverse_dep_tree
  → walk dependency/dependent graph → render tree
```

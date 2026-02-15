# Architecture

## Overview

wright is a Rust workspace producing three binaries from a single shared library crate. The library (`src/lib.rs`) exposes all core functionality as modules, and the binaries are thin CLI frontends.

## Binaries

| Binary | Entry Point | Description |
|--------|-------------|-------------|
| `wright` | `src/bin/wright.rs` | Package manager — install, remove, query, verify |
| `wright-build` | `src/bin/wright_build.rs` | Build tool — parse package.toml, run lifecycle, create archives |
| `wright-repo` | `src/bin/wright_repo.rs` | Repository tool — generate index (placeholder) |

## Module Map

```
src/
├── lib.rs                      # Library root — re-exports all modules
├── error.rs                    # Error types (WrightError, thiserror)
├── config.rs                   # GlobalConfig — loads /etc/wright/wright.toml
├── package/
│   ├── mod.rs
│   ├── manifest.rs             # PackageManifest — package.toml deserialization + validation
│   ├── version.rs              # Version parsing and comparison (semver)
│   └── archive.rs              # Binary package creation (tar.zst with .PKGINFO/.FILELIST)
├── builder/
│   ├── mod.rs                  # Builder — orchestrates the build pipeline
│   ├── lifecycle.rs            # LifecyclePipeline — stage ordering and execution
│   ├── executor.rs             # Executor — loads executor definitions, runs scripts
│   └── variables.rs            # Variable substitution engine (${PKG_NAME}, etc.)
├── database/
│   ├── mod.rs                  # Database — SQLite connection and queries
│   └── schema.rs               # Schema creation (packages, files, dependencies, transactions)
├── transaction/
│   ├── mod.rs                  # install_package, remove_package, verify_package
│   └── rollback.rs             # Rollback support for failed operations
├── resolver/
│   ├── mod.rs
│   ├── graph.rs                # Dependency graph construction
│   └── topo.rs                 # Topological sort
├── sandbox/
│   ├── mod.rs
│   └── bwrap.rs                # Bubblewrap command generation
├── repo/
│   ├── mod.rs
│   ├── index.rs                # Repository index parsing
│   ├── sync.rs                 # Remote repository synchronization
│   └── source.rs               # Source resolution (priority-based)
└── util/
    ├── mod.rs
    ├── download.rs             # HTTP downloads
    ├── checksum.rs             # SHA-256 computation and verification
    └── compress.rs             # tar.zst compression/decompression
```

## Architecture Layers

```
┌─────────────────────────────────────────────────────┐
│                    CLI Interface                      │
│              (wright / wright-build / wright-repo)          │
├─────────────────────────────────────────────────────┤
│                   Core Logic Layer                    │
│  ┌──────────────┬──────────────┬──────────────────┐ │
│  │   Resolver    │  Transaction │     Builder      │ │
│  │ (dependency   │  (atomic     │  (build          │ │
│  │  resolution)  │   ops)       │   pipeline)      │ │
│  └──────────────┴──────────────┴──────────────────┘ │
├─────────────────────────────────────────────────────┤
│                  Subsystem Layer                      │
│  ┌──────────┬──────────┬──────────┬───────────────┐ │
│  │ Database │ Sandbox  │ Executor │  Utilities     │ │
│  │ (SQLite) │ (bwrap)  │ (plugin) │  (download,    │ │
│  │          │          │          │   checksum,     │ │
│  │          │          │          │   compress)     │ │
│  └──────────┴──────────┴──────────┴───────────────┘ │
├─────────────────────────────────────────────────────┤
│                System Interface Layer                 │
│       (filesystem, namespace, process, network)      │
└─────────────────────────────────────────────────────┘
```

## Data Flow: Building a Package

```
package.toml
    │
    ▼
PackageManifest::from_file()       Parse and validate TOML
    │
    ▼
Builder::build()                   Create build directories (src/, pkg/, log/)
    │
    ▼
Builder::fetch() + verify()        Download and verify source archives
    │
    ▼
Builder::extract()                 Extract archives, detect BUILD_DIR
    │
    ▼
Builder::fetch_patches()           Download/copy patches into patches_dir
    │
    ▼
Builder::apply_patches()           Apply all patches with patch -Np1
    │
    ▼
variables::standard_variables()    Prepare ${PKG_NAME}, ${SRC_DIR}, etc.
    │
    ▼
LifecyclePipeline::new()           Determine stage order (default or custom)
    │
    ▼
LifecyclePipeline::run()           For each stage:
    │                                1. Substitute variables in script
    │                                2. Load executor definition
    │                                3. Write script to temp file
    │                                4. Execute (optionally inside bwrap sandbox)
    │                                5. Check exit code
    ▼
archive::create_archive()          Pack pkg/ into .wright.tar.zst with metadata
    │
    ▼
{name}-{version}-{release}-{arch}.wright.tar.zst
```

## Data Flow: Installing a Package

```
package.wright.tar.zst
    │
    ▼
transaction::install_package()
    │
    ├─► Extract archive to temp directory
    │
    ├─► Parse .PKGINFO metadata
    │
    ├─► Database::open() → BEGIN TRANSACTION
    │     ├─► Insert into packages table
    │     ├─► Insert file manifest into files table
    │     └─► Insert into dependencies table
    │
    ├─► Copy files to root directory
    │
    ├─► COMMIT TRANSACTION
    │
    └─► Print success / on error: rollback (delete files, ROLLBACK)
```

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI argument parsing |
| `serde` + `toml` | TOML deserialization |
| `rusqlite` (bundled) | SQLite database |
| `sha2` | SHA-256 checksums |
| `tar` + `zstd` | Archive creation/extraction |
| `walkdir` | Directory traversal |
| `tempfile` | Temporary build files |
| `anyhow` / `thiserror` | Error handling |
| `tracing` | Structured logging |
| `chrono` | Timestamps |
| `regex` | Package name validation |

## Implementation Status

The project is currently at Phase 1 (MVP). Implemented:

- package.toml parser with full validation
- Shell executor with variable substitution
- Lifecycle pipeline (all stages)
- Archive creation (tar.zst with .PKGINFO, .FILELIST)
- SQLite database with schema (packages, files, dependencies, transactions)
- Transactional install and remove with rollback
- `wright` CLI: install, remove, list, query, search, files, owner, verify
- `wright-build` CLI: build, --lint, --clean, --rebuild, --stage

Not yet implemented (Phase 2+):
- Bubblewrap sandbox integration
- Automatic dependency resolution
- Remote repository sync
- `wright-repo` functionality

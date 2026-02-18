# Architecture

Wright is designed as a modular toolkit split into two primary binaries that share a core library.

## Binary Split

- **`wright`**: The System Administrator. Focuses on the state of the live system, the SQLite database, and the lifecycle of installed packages.
- **`wbuild`**: The Plan Constructor. Focuses on transforming source code into binary packages using declarative `plan.toml` files.

## Module Map

```
src/
├── bin/
│   ├── wright.rs                   # System management CLI
│   └── wbuild.rs                   # Build system CLI
├── lib.rs                          # Library root
├── config.rs                       # GlobalConfig and AssembliesConfig
├── package/
│   ├── manifest.rs                 # plan.toml parsing, validation, replaces/conflicts
│   ├── version.rs                  # Version comparison logic
│   └── archive.rs                  # .wright.tar.zst creation and PKGINFO management
├── builder/
│   ├── mod.rs                      # The Build engine
│   ├── orchestrator.rs             # Multi-target scheduling, upward/downward recursion
│   ├── lifecycle.rs                # Stage execution pipeline
│   ├── executor.rs                 # Script execution (Shell, Python, etc.)
│   └── variables.rs                # Variable substitution engine
├── database/
│   ├── mod.rs                      # SQLite interface, integrity checks, shadowing records
│   └── schema.rs                   # Database schema and migrations
├── transaction/
│   ├── mod.rs                      # Atomic install/remove/upgrade with replacement support
│   └── rollback.rs                 # Journal-based rollback
├── query/
│   └── mod.rs                      # Analysis tools (health checks, tree rendering)
├── sandbox/                        # Isolation layers
└── util/                           # Helpers (checksum, compress, download)
```

## Data Flow: Build (wbuild)

```
plan.toml → PackageManifest
  → wbuild run
      → resolve targets → expand missing deps (Upward) → expand transitive rebuilds (Downward)
      → display Construction Plan
      → parallel execution:
          → Builder::build() → sandbox → executor
          → archive::create_archive() → PKGINFO (with link/replaces/conflicts)
      → if --install: acquisition of serial install lock → transaction::install_package()
```

## Data Flow: Management (wright)

```
.wright.tar.zst → transaction::install_package()
  → parse .PKGINFO → handle replaces (auto-uninstall) → check conflicts
  → BEGIN TRANSACTION → insert files → record shadows (ownership overlaps)
  → copy files to root → COMMIT

wright remove
  → check link-dependents → block if CRITICAL
  → check file shadows → preserve files if shared by other packages
```
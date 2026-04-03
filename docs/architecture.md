# Architecture

Wright is designed as a modular toolkit split into three binaries that share a core library.

The project uses a deliberate vocabulary:

- a `plan.toml` is a **plan**
- a built `.wright.tar.zst` is a **part**
- a repository stores finished parts
- the live machine is the **system**

See [terminology.md](terminology.md) for the canonical glossary.

## Binary Split

| Binary | Role | Domain |
|--------|------|--------|
| **`wbuild`** | Part Constructor | Transforms `plan.toml` → `.wright.tar.zst` via sandboxed builds |
| **`wrepo`** | Repository Manager | Manages indices (`wright.index.toml`) and source configuration |
| **`wright`** | System Administrator | Manages installed parts, the SQLite database, and system health |

Each binary owns a distinct phase of the part lifecycle. They share the
core library (`src/lib.rs`) but never overlap in responsibility:

```
  wbuild                    wrepo                      wright
 ┌────────────────┐     ┌──────────────────┐     ┌──────────────────┐
 │ plan.toml      │     │ .wright.tar.zst  │     │ wright.index.toml│
 │      ↓         │     │      ↓           │     │      ↓           │
 │ resolve deps   │     │ scan archives    │     │ resolve by name  │
 │ sandbox build  │────►│ generate index   │────►│ install/upgrade  │
 │ create archive │     │ manage sources   │     │ track in database│
 └────────────────┘     └──────────────────┘     └──────────────────┘
      builder/               repo/                 transaction/
      dockyard/                                    database/
      plan/                                        query/
```

## Module Map

```
src/
├── bin/
│   ├── wright.rs                   # System management CLI
│   ├── wbuild.rs                   # Build system CLI
│   └── wrepo.rs                    # Repository management CLI
├── lib.rs                          # Library root
├── config.rs                       # GlobalConfig and AssembliesConfig
├── plan/
│   └── manifest.rs                 # plan.toml parsing, validation, replaces/conflicts
├── part/
│   ├── version.rs                  # Version comparison logic
│   ├── archive.rs                  # .wright.tar.zst creation and PARTINFO management
│   └── fhs.rs                      # Filesystem Hierarchy Standard validation
├── builder/
│   ├── mod.rs                      # The Build engine
│   ├── orchestrator.rs             # Multi-target scheduling, upward/downward recursion
│   ├── lifecycle.rs                # Stage execution pipeline
│   ├── executor.rs                 # Script execution (Shell, Python, etc.)
│   ├── variables.rs                # Variable substitution engine
│   └── mvp.rs                      # MVP phase handling for cycle breaking
├── database/
│   ├── mod.rs                      # SQLite interface, integrity checks, shadowing records
│   └── schema.rs                   # Database schema and migrations
├── transaction/
│   ├── mod.rs                      # Atomic install/remove/upgrade with replacement support
│   └── rollback.rs                 # Journal-based rollback
├── repo/
│   ├── mod.rs                      # Repository types
│   ├── index.rs                    # Index generation and reading (wright.index.toml)
│   └── source.rs                   # Source configuration, resolver, version picking
├── query/
│   └── mod.rs                      # Analysis tools (health checks, tree rendering)
├── dockyard/                       # Sandbox isolation (bubblewrap)
└── util/                           # Helpers (checksum, compress, download)
```

### Which binary uses which modules

| Module | `wbuild` | `wrepo` | `wright` |
|--------|:--------:|:-------:|:--------:|
| `builder/` | primary | — | — |
| `dockyard/` | primary | — | — |
| `plan/` | primary | — | — |
| `repo/index` | — | primary | read-only |
| `repo/source` | — | primary | read-only |
| `database/` | read-only | read-only | primary |
| `transaction/` | via `-i` | — | primary |
| `part/archive` | create | scan | extract |
| `query/` | — | — | primary |
| `config` | full | partial | full |

## Data Flow: Build (wbuild)

```
plan.toml → PlanManifest
  → wbuild run
      → resolve targets
      → expand missing deps (Upward, build ops only)
      → expand transitive rebuilds (Downward, build ops only)
      → detect dependency cycles (Tarjan SCC)
          → if cycle found and MVP overrides declared (inline [mvp.dependencies] or sibling mvp.toml):
              inject two-pass plan ({pkg}:bootstrap build:mvp → rest → {pkg}:full build:full)
          → if cycle found and no MVP overrides: error with cycle description
      → log scheduling plan (build / relink / rebuild / build:mvp / build:full)
          (suppressed with --quiet; subprocess output echoed only with --verbose and single job)
      → parallel execution (topology-ordered):
          → MVP pass: Builder::build() with WRIGHT_BUILD_PHASE=mvp, no cache write
          → full pass: Builder::build() force=true, normal cache
          → archive::create_archive() → PARTINFO (runtime deps + relations)
      → output: .wright.tar.zst archives in components_dir
      → if --install:
          → create a session-local overlay sysroot + temporary parts.db snapshot
          → run dockyards against that stable session root instead of host /
          → stage each completed package into the session root between build waves
          → defer install/upgrade hooks during session staging
          → after all tasks succeed, commit staged packages to host / in order
          → run deferred hooks on the final host-root commit
```

### Dependency cascade rules

`wbuild resolve` performs dependency-driven expansion and outputs plan names for piping into `wbuild run`. Scope flags (`--self`, `--deps`, `--dependents`) are composable; `--deps=all` and `--dependents=all` are force-rebuild escalators that extend the scope to already-installed or non-link dependents. `wbuild run` is a pure builder — it builds exactly the targets it receives. `checksum`, `fetch`, and `check` skip all expansion entirely.

See [dependencies.md](dependencies.md) for the conceptual model and [cli-reference.md](cli-reference.md) for the full flag reference.

## Data Flow: Index (wrepo)

```
wrepo sync [dir]
  → scan dir for .wright.tar.zst files
  → for each archive: extract .PARTINFO metadata
  → collect: name, version, release, epoch, arch, description,
             dependencies, provides, conflicts, replaces,
             filename, sha256, install_size
  → write wright.index.toml

wrepo source add/remove/list
  → read/write /etc/wright/repos.toml

wrepo list/search
  → read wright.index.toml from all configured sources
  → cross-reference with installed database for [installed] tags
```

## Data Flow: Management (wright)

```
wright install <name>
  → resolver reads wright.index.toml from configured sources
  → picks latest version (or user-specified version)
  → locates .wright.tar.zst archive on disk

.wright.tar.zst → transaction::install_package()
  → parse .PARTINFO → handle replaces (auto-uninstall) → check conflicts
  → BEGIN TRANSACTION → insert files → record shadows (ownership overlaps)
  → copy files to root → COMMIT

wright remove
  → check link-dependents → block if CRITICAL
  → check file shadows → preserve files if shared by other parts

wright remove --cascade
  → compute orphan dependencies (origin = 'dependency', not needed by others)
  → remove target → remove orphan deps leaf-first
```

## Cross-Tool Coordination

The three tools coordinate through **shared file formats**, not direct
communication:

| Artifact | Written by | Read by | Location |
|----------|-----------|---------|----------|
| `plan.toml` | user | `wbuild` | `plans_dir` |
| `.wright.tar.zst` | `wbuild` | `wrepo`, `wright` | `components_dir` |
| `wright.index.toml` | `wrepo` | `wright` | alongside archives |
| `/etc/wright/repos.toml` | `wrepo` | `wright`, `wrepo` | system config |
| `parts.db` (SQLite) | `wright` | `wbuild` (read), `wrepo` (read) | `db_path` |
| `repo.db` (SQLite) | `wrepo`, `wbuild` | `wright`, `wrepo` | `repo_db_path` |

`wbuild` reads the database to check which parts are already installed
(for dependency expansion and session planning). With `-i`, it first writes
to a temporary session-local database copy, then commits successful staged
packages to the host database at the end of the run. `wrepo` reads the
database to annotate `[installed]` tags in listings.

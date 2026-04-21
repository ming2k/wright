# Database Design (v3.0.0)

Wright uses two distinct SQLite databases to manage the lifecycle of parts, from their creation to their deployment on the live system. Starting with v3.0.0, both databases use a unified SQL-based migration system.

## Overview

| Database | Filename | Scope | Primary Role |
|----------|----------|-------|--------------|
| **Installed DB** | `installed.db` | System-wide | Authoritative state of the live system. Tracks deployed files and ownership. |
| **Archive DB** | `archives.db` | Local cache | Catalogue of locally built archives (`.wright.tar.zst`). Speeds up dependency resolution. |

---

## Migration System

Wright v3.0.0 introduces a structured migration system using standard `.sql` files.

- **Schema Location**: Migrations are stored in `src/database/migrations/`.
- **Initialization**: Databases are automatically initialized on the first run.
- **Upgrading**: The system uses SQLx's `_sqlx_migrations` table to track and apply pending migrations.
- **2.x -> 3.x Migration**: A dedicated tool `final_migration.py` is provided in the project root to migrate existing databases to the new v3 format.

---

## 1. Installed Database (`installed.db`)

Located at `/var/lib/wright/state/installed.db` by default.

This database represents the ground truth of the operating system's state as managed by Wright. Every file deployed to the root filesystem is recorded here.

### Key Tables

- **`parts`**: Metadata of all currently installed parts (name, version, release, epoch, origin, etc.).
- **`files`**: A complete manifest of every file, symlink, and directory owned by each part, including SHA-256 hashes for integrity verification.
- **`dependencies`**: The runtime and link-time dependency graph of the installed system. Used for cascade removal and orphan detection.
- **`provides`, `conflicts`, `replaces`**: Metadata used to handle virtual packages and ensure system consistency during upgrades.
- **`transactions`**: A log of all install, upgrade, and remove operations, allowing for rollback in case of failure.
- **`shadowed_files`**: Tracks file collisions where one part's file is "diverted" or overwritten by another, ensuring safe restoration when the shadowing part is removed.
- **`build_sessions`**: Tracks progress of multi-package `apply` runs to support the `--resume` feature.

---

## 2. Archive Database (`archives.db`)

Located at `/var/lib/wright/state/archives.db` by default. Formerly known as the "inventory database."

This database acts as a local metadata cache for built parts. Without this database, Wright would need to unpack every `.wright.tar.zst` file in the `parts_dir` just to find out its dependencies or version.

### Key Tables

- **`parts`**: Metadata and physical filename of built archives. This maps a logical part name and version to a specific `.wright.tar.zst` file on disk.
- **`dependencies`**: Caches the runtime dependencies of each archive. This allows `wright install` to calculate the full dependency tree of a new target in milliseconds.
- **`provides`, `conflicts`, `replaces`**: Pre-indexed metadata from the archives. This allows the resolver to detect conflicts or find virtual package providers *before* any installation begins.

### Lifecycle

- **Registration**: When `wright build` completes, it extracts the `.PARTINFO` from the new archive and registers it here.
- **Pruning**: `wright prune` removes physical archive files and their corresponding rows in this database simultaneously.
- **Resolution**: `wright install <name>` queries this database to find the latest version and its requirements.

---

## Rationale: Why two databases?

1. **Performance**: Separating the "Catalogue" (`archives.db`) from the "State" (`installed.db`) ensures that the resolver can compute complex build and install plans without being bogged down by the thousands of file-level records in the installed database.
2. **Resilience**: The `installed.db` must be kept extremely consistent. By keeping the transient "local build inventory" in a separate file, we reduce the risk of corruption during heavy build/prune cycles affecting the system's ability to boot or manage its own state.
3. **Portability**: The `archives.db` can theoretically be shared or synced (as a "repository index"), whereas the `installed.db` is unique to every specific machine.

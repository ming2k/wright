# Database Design

## Databases

| Database | Default path | Scope | Role |
|----------|--------------|-------|------|
| System DB | `/var/lib/wright/wright.db` | target root | Installed state, file ownership, dependencies, transactions |

## Archive Metadata

| Artifact | Default path | Lookup method | Role |
|----------|--------------|---------------|------|
| Part archives | `/var/lib/wright/parts/*.wright.tar.zst` | scan `parts_dir` and read `.PARTINFO` | Local archive inventory for install, upgrade, sysupgrade,  |

## Migration System

| Item | Value |
|------|-------|
| Migration files | `src/database/migrations/*.sql` |
| Migration tracker | SQLx `_sqlx_migrations` table |
| Initialization | automatic on database open |
| Upgrade | pending migrations run automatically |
| Immutable history | never edit files under `src/database/migrations/` |

## Tables

| Table | Contents |
|-------|----------|
| `plans` | plan identity metadata (name, version, release, epoch, arch) |
| `parts` | installed part metadata: origin, plan association, archive hash |
| `files` | installed file paths, types, checksums, ownership |
| `dependencies` | advisory runtime dependency edges per part (soft TEXT pointer; not enforced) |
| `conflicts` | mutually exclusive part name declarations |
| `replaces` | rename / supersession metadata |
| `shadowed_files` | file collision records used for divert and safe removal |
| `history` | permanent audit log of install, upgrade, remove actions |
| `delivery_transactions` | **Temporary WAL**: user-invoked delivery command status (cleaned after commit/rollback) |
| `transaction_ops` | **Temporary WAL**: per-DAG-node deploy actions (cleaned after commit/rollback) |

Build deps, link deps, and `provides` are deliberately not persisted. See
[Dependency Philosophy](../explanation/dependency-philosophy.md) and
[ADR-0016](../adr/0016-advisory-runtime-dependencies.md).

## Entity Relationship Diagram

```mermaid
erDiagram
    plans ||--o{ parts : contains
    parts ||--o{ files : owns
    parts ||--o{ dependencies : "runtime deps (advisory)"
    parts ||--o{ conflicts : conflicts
    parts ||--o{ replaces : replaces
    parts ||--o{ shadowed_files : "original owner"
    parts ||--o{ shadowed_files : "shadowed by"
    delivery_transactions ||--o{ transaction_ops : contains
    plans {
        INTEGER id PK
        TEXT name UK
        TEXT version
        INTEGER release
        INTEGER epoch
        TEXT arch
        DATETIME registered_at
    }

    parts {
        INTEGER id PK
        TEXT name UK
        INTEGER plan_id FK
        DATETIME installed_at
        TEXT part_hash
        TEXT install_scripts
        TEXT origin
    }

    files {
        INTEGER id PK
        INTEGER part_id FK
        TEXT path
        TEXT file_hash
        TEXT file_type
        INTEGER file_mode
        INTEGER file_size
        BOOLEAN is_config
    }

    dependencies {
        INTEGER id PK
        INTEGER part_id FK
        TEXT depends_on
        TEXT version_constraint
    }

    conflicts {
        INTEGER id PK
        INTEGER part_id FK
        TEXT name
    }

    replaces {
        INTEGER id PK
        INTEGER part_id FK
        TEXT name
    }

    shadowed_files {
        INTEGER id PK
        TEXT path
        INTEGER original_owner_id FK
        INTEGER shadowed_by_id FK
        TEXT diverted_to
        DATETIME timestamp
    }

    history {
        INTEGER id PK
        DATETIME timestamp
        TEXT session_id
        TEXT command
        TEXT part_name
        TEXT action
        TEXT old_version
        TEXT new_version
        TEXT old_hash
        TEXT new_hash
        TEXT status
        TEXT details
    }

    delivery_transactions {
        INTEGER id PK
        TEXT command
        TEXT status
        DATETIME created_at
        DATETIME updated_at
    }

    transaction_ops {
        INTEGER id PK
        INTEGER transaction_id FK
        TEXT part_name
        TEXT part_hash
        TEXT action_type
        INTEGER execution_order
        TEXT status
        TEXT old_hash
        TEXT error_msg
    }
```

## Key Constraints

| Table | Constraint | Rationale |
|-------|------------|-----------|
| `parts.name` | `UNIQUE` | Part names are globally unique identifiers |
| `parts.origin` | `CHECK(origin IN ('dependency','build','manual','external'))` | Enforces valid provenance values at the DB layer |
| `plans.name` | `UNIQUE` | Each plan name maps to exactly one plan record |
| `transaction_ops.transaction_id` | `REFERENCES delivery_transactions(id)` | Operations belong to one delivery (temporary) |

## Non-Foreign-Key References

| Field | References | Purpose |
|-------|------------|---------|
| `dependencies.depends_on` | `parts.name` (or `replaces.name`) | Advisory runtime-dependency target. Soft pointer — target may be unresolved (treated as "unsatisfied" rather than an error). |
| `history.part_name` | `parts.name` at transaction time | Historical install, upgrade, remove subject |
| `history.session_id` | `delivery_transactions.id` (legacy) | Logical grouping for history records |

## Removed Databases

| Removed artifact | Current replacement |
|------------------|---------------------|
| `archives.db` | direct `parts_dir` scan plus `.PARTINFO` reads |
| `installed.db` default name | `/var/lib/wright/wright.db` |

# Database Design

## Databases

| Database | Default path | Scope | Role |
|----------|--------------|-------|------|
| System DB | `/var/lib/wright/wright.db` | target root | Installed state, file ownership, dependencies, transactions, workflow tracking |

## Archive Metadata

| Artifact | Default path | Lookup method | Role |
|----------|--------------|---------------|------|
| Part archives | `/var/lib/wright/parts/*.wright.tar.zst` | scan `parts_dir` and read `.PARTINFO` | Local archive inventory for install, upgrade, sysupgrade, apply, and prune |

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
| `plans` | plan metadata (name, version, release, epoch, description, arch, license) |
| `parts` | installed part metadata: origin, plan association, archive hash |
| `files` | installed file paths, types, checksums, ownership |
| `dependencies` | advisory runtime dependency edges per part (soft TEXT pointer; not enforced) |
| `conflicts` | mutually exclusive part name declarations |
| `replaces` | rename / supersession metadata |
| `shadowed_files` | file collision records used for divert and safe removal |
| `transactions` | install, upgrade, remove history |
| `workflows` | active content-addressed workflow identities for incomplete work |
| `workflow_steps` | active per-step resume state for incomplete workflows |

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
    workflows ||--o{ workflow_steps : contains
    plans {
        INTEGER id PK
        TEXT name UK
        TEXT version
        INTEGER release
        INTEGER epoch
        TEXT description
        TEXT arch
        TEXT license
        TEXT url
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

    transactions {
        INTEGER id PK
        DATETIME timestamp
        TEXT operation
        TEXT part_name
        TEXT old_version
        TEXT new_version
        TEXT status
        TEXT backup_path
    }

    workflows {
        TEXT id PK
        TEXT kind
        TEXT inputs_json
        INTEGER created_at
    }

    workflow_steps {
        TEXT id PK
        TEXT workflow_id FK
        TEXT kind
        TEXT inputs_json
        TEXT depends_on_json
        TEXT status
        INTEGER attempt
        TEXT outputs_json
        TEXT failure_json
        INTEGER started_at
        INTEGER finished_at
    }

```

## Key Constraints

| Table | Constraint | Rationale |
|-------|------------|-----------|
| `parts.name` | `UNIQUE` | Part names are globally unique identifiers |
| `parts.origin` | `CHECK(origin IN ('dependency','build','manual','external'))` | Enforces valid provenance values at the DB layer |
| `plans.name` | `UNIQUE` | Each plan name maps to exactly one plan record |
| `workflows.id` | `PRIMARY KEY` | Active content-addressed workflow IDs are immutable |
| `workflow_steps.id` | `PRIMARY KEY` | Content-addressed step IDs are immutable |
| `workflow_steps.status` | `CHECK(status IN ('pending','running','succeeded','failed','skipped'))` | Enforces valid step lifecycle states |

## Non-Foreign-Key References

| Field | References | Purpose |
|-------|------------|---------|
| `dependencies.depends_on` | `parts.name` (or `replaces.name`) | Advisory runtime-dependency target. Soft pointer — target may be unresolved (treated as "unsatisfied" rather than an error). |
| `transactions.part_name` | `parts.name` at transaction time | Historical install, upgrade, remove subject |

## Removed Databases

| Removed artifact | Current replacement |
|------------------|---------------------|
| `archives.db` | direct `parts_dir` scan plus `.PARTINFO` reads |
| `installed.db` default name | `/var/lib/wright/wright.db` |

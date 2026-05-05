# Database Design

## Databases

| Database | Default path | Scope | Role |
|----------|--------------|-------|------|
| System DB | `/var/lib/wright/wright.db` | target root | Installed state, file ownership, dependencies, transactions, execution sessions |

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
| `plan_build_deps` | build-time dependency edges per plan |
| `plan_link_deps` | ABI-sensitive link dependency edges per plan |
| `parts` | installed part metadata: origin, plan association, archive hash |
| `files` | installed file paths, types, checksums, ownership |
| `dependencies` | installed runtime dependency edges per part |
| `provides` | virtual capabilities a part provides |
| `conflicts` | mutually exclusive part name declarations |
| `replaces` | automatic replacement metadata |
| `optional_dependencies` | informational (non-enforced) optional dependency hints |
| `shadowed_files` | file collision records used for divert and safe removal |
| `transactions` | install, upgrade, remove history |
| `execution_sessions` | resumable `build` and `apply` session metadata |
| `execution_session_items` | per-task resume state within a session |
| `build_sessions` | per-plan build progress state for `--resume` |

## Key Constraints

| Table | Constraint | Rationale |
|-------|------------|-----------|
| `parts.name` | `UNIQUE` | Part names are globally unique identifiers |
| `parts.origin` | `CHECK(origin IN ('dependency','build','manual','external'))` | Enforces valid provenance values at the DB layer |
| `plans.name` | `UNIQUE` | Each plan name maps to exactly one plan record |

## Removed Databases

| Removed artifact | Current replacement |
|------------------|---------------------|
| `archives.db` | direct `parts_dir` scan plus `.PARTINFO` reads |
| `installed.db` default name | `/var/lib/wright/wright.db` |

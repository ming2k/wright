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
| `plans` | plan metadata, plan version, plan dependency JSON |
| `parts` | installed part metadata, origin, plan association |
| `files` | installed file paths, kinds, checksums, ownership |
| `dependencies` | installed runtime dependency edges |
| `provides` | virtual capabilities |
| `conflicts` | mutually exclusive part names |
| `replaces` | automatic replacement metadata |
| `transactions` | install, upgrade, remove history |
| `shadowed_files` | file collision restoration records |
| `execution_sessions` | resumable `build` and `apply` session metadata |
| `execution_session_items` | per-task resume state |

## Removed Databases

| Removed artifact | Current replacement |
|------------------|---------------------|
| `archives.db` | direct `parts_dir` scan plus `.PARTINFO` reads |
| `installed.db` default name | `/var/lib/wright/wright.db` |

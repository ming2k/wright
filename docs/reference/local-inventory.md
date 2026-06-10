# Local Part Inventory

Wright stores built parts as `.wright.tar.zst` archives in `parts_dir` (default: `/var/lib/wright/parts`). There is no separate index or catalogue database.

## Current Model

- `wright build` builds plans into staging directories (`work/` and `staging/`), then seals outputs into `.wright.tar.zst` archives in `parts_dir`
- `wright merge` resolves plan names to expected archives in `parts_dir` and deploys them
- `wright merge --path` reads explicit archive paths and their `.PARTINFO` metadata
- `wright merge` rejects mixed-revision archives from the same plan in one batch
- `wright merge` rejects plan revision changes that would leave installed outputs from the old revision
- `wright install` performs the full lifecycle: resolve, forge, seal, and merge in one command

## Quick Start

```bash
wright build curl
wright merge curl
```

Or use `wright install` for plan-driven maintenance:

```bash
wright install curl
```

## Part Archive Metadata

Every `.wright.tar.zst` carries metadata files at the archive root:

| File | Contents |
|------|----------|
| `.PARTINFO` | TOML part metadata (sections below) |
| `.FILELIST` | one absolute installed path per line |
| `.HOOKS` | deploy hooks in TOML (optional; only when the plan declares hooks) |

### `.PARTINFO` sections

| Section | Fields | Presence |
|---------|--------|----------|
| `[part]` | `name`, `build_date`, `packager`, `runtime_deps` | always |
| `[relations]` | `replaces`, `conflicts` | only when declared |
| `[backup]` | `files` | only when declared |
| `[plan]` | `name`, `version`, `release`, `epoch`, `arch` | always |
| `[provenance]` | see below | absent on parts sealed before ADR-0023 |

### `[provenance]` fields

| Field | Content |
|-------|---------|
| `plan_checksum` | SHA-256 hex of the raw `plan.toml` that produced the part (`mvp.toml` overlay excluded) |
| `source_checksums` | array of `<kind> <locator> <verification>` strings, one per `[[sources]]` entry, `${VAR}` expanded |
| `wright_version` | version of the `wright` binary that sealed the part |
| `isolation` | weakest isolation level declared across the plan's pipeline stages (`none` / `relaxed` / `strict`) |

Provenance is descriptive, never enforced; `wright doctor` uses
`plan_checksum` to report drift between installed parts and current plan
source. See [ADR-0023](../adr/0023-parts-as-maintenance-ledger.md).

## Low-Level Pipeline

For explicit control over build and merge phases:

```bash
wright build --force zlib openssl
wright merge zlib openssl
```

`--print-parts` keeps stdout reserved for archive paths. Human-readable logs stay on stderr.

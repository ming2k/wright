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

## Cleaning Old Parts

Use `wright prune` to clean the parts directory:

```bash
wright prune --latest --apply
```

- `--latest` keeps only the newest archive per part name while preserving installed versions
- `--apply` performs deletions; otherwise prints a dry-run report

## Low-Level Pipeline

For explicit control over build and merge phases:

```bash
wright build --force zlib openssl
wright merge zlib openssl
```

`--print-parts` keeps stdout reserved for archive paths. Human-readable logs stay on stderr.

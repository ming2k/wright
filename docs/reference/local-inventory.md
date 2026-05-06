# Local Part Inventory

Wright stores built parts as `.wright.tar.zst` archives in `parts_dir` (default: `/var/lib/wright/parts`). There is no separate index or catalogue database.

## Current Model

- `wright build` builds plans into staging directories (`work/` and `staging/`)
- `wright package` slices `staging/` into `outputs/` (using the plan's `[[output]]` rules) and creates `.wright.tar.zst` archives
- `wright install` resolves plan names to expected archives in `parts_dir`
- `wright install --path` reads explicit archive paths and their `.PARTINFO` metadata
- `wright install` rejects mixed-revision archives from the same plan in one batch
- `wright install` rejects plan revision changes that would leave installed outputs from the old revision

## Quick Start

```bash
wright build curl
wright package curl
wright install curl
```

Or use `wright apply` for plan-driven maintenance:

```bash
wright apply curl
```

## Cleaning Old Parts

Use `wright prune` to clean the parts directory:

```bash
wright prune --latest --apply
```

- `--latest` keeps only the newest archive per part name while preserving installed versions
- `--apply` performs deletions; otherwise prints a dry-run report

## Low-Level Pipeline

For explicit control over build and install phases:

```bash
wright resolve openssl --rdeps=all --depth=0 > /tmp/wright-rebuild
wright build --force $(cat /tmp/wright-rebuild)
wright package --print-parts $(cat /tmp/wright-rebuild) | wright install --path
```

`--print-parts` keeps stdout reserved for archive paths. Human-readable logs stay on stderr.

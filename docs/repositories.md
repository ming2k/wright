# Local Part Inventory

This document keeps the old path name for compatibility, but Wright no longer
has a separate publish/index layer.

## Current Model

- `wright build` builds `.wright.tar.zst` parts into `parts_dir`
- `wright build` registers each successful build in `inventory_db_path`
- `wright` resolves part names from that local inventory

There is no separate indexing tool, no source list, and no sync step.

## Quick Start

```bash
wright build curl
wright install curl
```

For plan-first maintenance, prefer:

```bash
wright apply @base
wright apply curl
```

`wright apply` checks the local inventory first, builds any missing or outdated
parts from plans, and then installs the requested outputs.

## Inventory Records

The local inventory stores metadata for built parts, including:

- name, version, release, epoch, architecture
- description and runtime dependency metadata from `.PARTINFO`
- part path and SHA-256
- the originating plan and build identity used to detect stale outputs

Multiple versions of the same part can exist in the inventory. `wright install`
and `wright upgrade` select from those locally registered versions.

## Cleaning Old parts

Use `wright prune` to reconcile the part store with the inventory:

```bash
wright prune --untracked
wright prune --latest --apply
```

- `--untracked` removes files present on disk but absent from `archives.db`
- `--latest` keeps only the newest tracked part per part name while
 preserving versions that are currently installed
- add `--apply` to perform deletions; otherwise Wright prints a dry-run report

## Low-Level Pipeline

If you want explicit control over build and install phases, print part paths
from `wright build` and pipe them into `wright install`:

```bash
wright resolve openssl --rdeps=all --depth=0 | wright build --force --print-parts | wright install
```

`--print-parts` keeps stdout reserved for part paths. Human-readable
logs, including live `-v` subprocess output, stay on stderr so this pipeline
remains safe.

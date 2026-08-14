# How to Estimate Build Costs

Wright records every build attempt in a per-plan ledger file:
`<ledger_dir>/<plan>/builds.jsonl` (default ledger_dir:
`/var/lib/wright/ledger`). Each line is one JSON document describing one
forge attempt — success or failure — so the history doubles as the cost
estimate for the next build or upgrade (ADR-0041).

## Read recent build times

```bash
tail -n 5 /var/lib/wright/ledger/curl/builds.jsonl | jq -r '.duration_secs'
```

Restrict to full builds (partial runs from `--stage`, `--fetch-only`, or
`--until-stage` are marked `full: false` and would skew the estimate):

```bash
jq -r 'select(.full and .success) | .duration_secs' \
  /var/lib/wright/ledger/curl/builds.jsonl
```

## Break a build down by step

The `stages` array lists each step that ran (`charge`, forge stages,
`slice`) with its seconds. Steps skipped by the checkpoint are absent, so a
cached rebuild records close to zero:

```bash
tail -n 1 /var/lib/wright/ledger/curl/builds.jsonl | jq '.stages'
```

## Estimate download and disk cost

- `source_bytes` — total size of the plan's cached source archives (what an
  upgrade downloads when the version bumps).
- `staging_bytes` / `staging_files` — the unpacked staging tree (installed
  footprint).

The sealed archive size is not duplicated into the record; `stat` the
archive under `parts_dir/<plan>/` directly.

## Check the failure rate

Failed attempts carry `success: false` and a flattened `error` chain:

```bash
jq -r 'select(.success | not) | .error' \
  /var/lib/wright/ledger/curl/builds.jsonl
```

## Compare across hardware changes

Every record embeds a host summary (`host.cpu_model`, `host.cpu_cores`,
`host.memory_bytes`, `host.kernel`) and the `wright_version` that ran the
build. When the machine changes, old records stay valid as history —
filter on `host.cpu_model` to compare like with like.

For the full platform forensics of a specific archive (including CPU
flags), read its `.BUILDINFO` member:

```bash
tar -xOf /var/lib/wright/parts/curl/curl-*.wright.tar.zst .BUILDINFO
```

## Notes

- Records are append-only; nothing rotates or prunes the ledger.
- Ledger writes are advisory: a build never fails because the record could
  not be written (e.g. non-root build against the system ledger).

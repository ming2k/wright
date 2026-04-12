# CLI Reference

Wright provides one CLI, `wright`, with top-level subcommands for both
build-side and system-side workflows.

## Global options

- `--config <PATH>`
- `--db <PATH>`
- `--root <PATH>`
- `-v`, `-vv`
- `--quiet`

## System commands

- `wright install <PART...>`
 installs part paths or locally registered part names
- `wright apply <TARGET...>`
 resolves plans or assemblies, executes wave-by-wave build/install orchestration, and converges the live system to those targets
- `wright upgrade <PART...>`
 upgrades installed parts by local part name or part path
- `wright sysupgrade`
 upgrades everything to the newest locally registered parts
- `wright remove <PART...>`
- `wright deps [PART]`
- `wright list`
- `wright query <PART>`
- `wright search <KEYWORD>`
- `wright files <PART>`
- `wright owner <FILE>`
- `wright verify [PART]`
- `wright doctor`
- `wright history [PART]`
- `wright assume <NAME> <VERSION>`
- `wright unassume <NAME>`
- `wright mark <PART...> --as-dependency|--as-manual`

## Build commands

- `wright build <TARGET...>`
 builds plans or assemblies
- `wright resolve <TARGET...>`
 expands dependency and rebuild scope without building
- `wright prune`
 cleans tracked or stray parts from the local inventory

Useful `wright build` flags:

- `--force`
- `--clean`
- `--resume [HASH]`
- `--stage <NAME>`
- `--skip-check`
- `--mvp`
- `--print-parts`
 prints only part paths on stdout; logs and progress stay on stderr for safe piping into `wright install`
- `--fetch`
- `--checksum`
- `--lint`

Useful `wright resolve` flags:

- ``
- `--deps [none|missing|sync|all]`
- `--rdeps [link|all]`
- `--depth <N>`
- `--tree`

Useful `wright apply` flags:

- `-f`, `--force`
- `--dry-run`


Useful `wright prune` flags:

- `--untracked`
- `--latest`
- `--apply`

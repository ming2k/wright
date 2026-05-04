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
 resolves plans, executes wave-by-wave build/install orchestration, and converges the live system to those targets with plan-driven install/upgrade handling
- `wright upgrade <PART...>`
 upgrades installed parts by local part name or part path
- `wright sysupgrade`
 upgrades everything to the newest locally registered parts
- `wright remove <PART...>`
- `wright list`
- `wright query <PART>`
- `wright search <KEYWORD>`
- `wright files <PART>`
- `wright owner <FILE>`
- `wright verify [PART]`
- `wright doctor`
- `wright history [PART]`
- `wright assume <NAME> <VERSION>`
  mark an external part as satisfied (useful for bootstrap)
- `wright assume --file <FILE>`
  bulk assume from a file with `name version` lines
- `echo "name version" | wright assume`
  pipe multiple parts to assume
- `wright unassume <NAME>`
  remove an assumed record
- `wright mark <PART...> --as-dependency|--as-manual`

## Build commands

- `wright build <TARGET...>`
 builds plans
- `wright resolve <TARGET...>`
 expands dependency and rebuild scope without building
- `wright prune`
 cleans tracked or stray parts from the local inventory

Useful `wright build` flags:

- `--force`
- `--clean`
- `--resume [HASH]`
- `--stage <NAME>`
- `--until-stage <NAME>`
- `--skip-check`
- `--mvp`
- `--print-parts`
 prints only part paths on stdout; logs and progress stay on stderr for safe piping into `wright install`
- `--fetch`
- `--checksum`

## Lint commands

- `wright lint [TARGET...]`
  validates plan syntax, logical integrity, and dependency graph cycles for
  specified plans (or all plans if omitted)

Useful `wright lint` flags:

- `-r`, `--recursive`
  Recurse into subdirectories when scanning for plans.

Useful `wright resolve` flags:

- `--exclude-targets`
- `--deps [link|runtime|build|all]`
- `--rdeps [link|runtime|build|all]`
- `--match [missing|outdated|installed|all]`
- `--depth <N>`
- `--tree`
- `--installed`

Useful `wright apply` flags:

- `--deps [link|runtime|build|all]`
- `--rdeps [link|runtime|build|all]`
- `--match [missing|outdated|installed|all]`
- `--depth <N>`
- `-f`, `--force`
- `--resume [HASH]`
- `--dry-run`


Useful `wright prune` flags:

- `--untracked`
- `--latest`
- `--apply`

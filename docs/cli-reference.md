# CLI Reference

Wright provides two commands:

- `wbuild` for building and maintaining local archives
- `wright` for mutating and inspecting the live system

## `wright`

Global options:

- `--root <PATH>`
- `--config <PATH>`
- `--db <PATH>`
- `-v`, `-vv`
- `--quiet`

Main commands:

- `wright install <PART...>`
  installs archive paths or locally registered part names
- `wright apply <TARGET...>`
  resolves plans or assemblies, executes wave-by-wave build/install orchestration, and converges the live system to those targets
- `wright upgrade <PART...>`
  upgrades installed parts by local part name or archive path
- `wright sysupgrade`
  upgrades everything to the newest locally registered archives
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

## `wbuild`

Global options:

- `--config <PATH>`
- `--db <PATH>`
- `-v`, `-vv`
- `--quiet`

Main commands:

- `wbuild run <TARGET...>`
  builds plans or assemblies
- `wbuild resolve <TARGET...>`
  expands dependency and rebuild scope without building
- `wbuild check <TARGET...>`
- `wbuild fetch <TARGET...>`
- `wbuild checksum <TARGET...>`
- `wbuild prune`

Useful `wbuild run` flags:

- `--force`
- `--clean`
- `--resume [HASH]`
- `--stage <NAME>`
- `--skip-check`
- `--mvp`
- `--dockyards <N>`
- `--print-archives`

Useful `wbuild prune` flags:

- `--untracked`
- `--latest`
- `--apply`

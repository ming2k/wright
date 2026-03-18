# wright

A declarative, extensible, sandboxed Linux system part manager for LFS-based distributions.

Wright intentionally uses a ship-maintenance metaphor: the live computer is the Ship of Theseus in motion, `plan.toml` defines how to manufacture one replacement **part**, and the finished `.wright.tar.zst` is that installable **part**.

## Features

- **Declarative TOML plans** — metadata, dependencies, sources, and build steps in a single `plan.toml`
- **Lifecycle pipeline** — ordered build stages with pre/post hooks and pluggable executors
- **Sandbox isolation** — namespace-isolated builds via bubblewrap
- **Transactional operations** — atomic install/remove with rollback
- **Binary parts** — `.wright.tar.zst` archives with embedded metadata

## Building

```
cargo build --release
```

Builds also generate man pages for `wright`, `wbuild`, and `wrepo` under `target/man/`.
Install them system-wide with:

```sh
install -Dm644 target/man/wright.1 /usr/share/man/man1/wright.1
install -Dm644 target/man/wbuild.1 /usr/share/man/man1/wbuild.1
install -Dm644 target/man/wrepo.1 /usr/share/man/man1/wrepo.1
```

## Quick Start

```bash
wbuild run hello                                      # build from plan
wrepo sync                                            # index built archives
wright install hello                                  # install by name
wright list                                           # list installed parts
wright remove hello                                   # remove
```

## Terminology

Wright uses a deliberate vocabulary so different stages of the workflow are not all called "packages":

- **Plan**: the `plan.toml` blueprint for building one part
- **Part**: the built `.wright.tar.zst` binary artifact
- **Assembly**: a build-time grouping of plans
- **Kit**: an install-time grouping of parts
- **Repository**: the indexed inventory of built parts
- **System**: the live machine being maintained

The guiding metaphor is the Ship of Theseus: Wright is about replacing parts on a ship that must keep sailing.
See [Terminology](docs/terminology.md) for the canonical definitions.

## Documentation

- [Getting Started](docs/getting-started.md) — prerequisites, building, first part
- [Usage Guide](docs/usage.md) — full workflow, build options, LFS chroot deployment
- [Terminology](docs/terminology.md) — canonical project vocabulary and metaphor
- [Writing Plans](docs/writing-plans.md) — complete `plan.toml` reference
- [CLI Reference](docs/cli-reference.md) — all commands and flags
- [Configuration](docs/configuration.md) — wright.toml, repos.toml, executors
- [Logging](docs/logging.md) — log locations, verbosity, and configuration
- [Maintenance Guide](docs/os-maintenance-guide.md) — OS package maintenance policy (not wright tool maintenance)
- [Architecture](docs/architecture.md) — code structure and module overview
- [Design Specification](docs/design-spec.md) — full technical design document
- [Contributing](docs/contributing.md) — development setup and conventions

## License

MIT

# wright

A declarative, extensible, sandboxed Linux package manager for LFS-based distributions.

## Features

- **Declarative TOML plans** — metadata, dependencies, sources, and build steps in a single `plan.toml`
- **Lifecycle pipeline** — ordered build stages with pre/post hooks and pluggable executors
- **Sandbox isolation** — namespace-isolated builds via bubblewrap
- **Transactional operations** — atomic install/remove with rollback
- **Binary packages** — `.wright.tar.zst` archives with embedded metadata

## Building

```
cargo build --release
```

## Quick Start

```
wright build hello                                    # build from plan
wright install hello-1.0.0-1-x86_64.wright.tar.zst   # install
wright list                                           # list installed
wright remove hello                                   # remove
```

## Documentation

- [Getting Started](docs/getting-started.md) — prerequisites, building, first package
- [Usage Guide](docs/usage.md) — full workflow, build options, LFS chroot deployment
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

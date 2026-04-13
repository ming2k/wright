# wright

A declarative, extensible, sandboxed Linux system part manager for LFS-based distributions.

Wright uses a ship-maintenance metaphor: `plan.toml` defines how to manufacture
one replacement **part**, the finished `.wright.tar.zst` is that installable
**part**, and the live machine is the ship that keeps sailing while parts are
replaced.

## Features

- Declarative TOML plans
- Sandboxed builds via bubblewrap
- Transactional install / upgrade / remove
- Local part inventory for reuse and rollback
- Source-first maintenance through wave-aware `wright apply`

## Build

```sh
cargo build --release
install -Dm644 target/man/wright.1 /usr/share/man/man1/wright.1
```

## Quick Start

```bash
wright build hello
wright install hello
wright apply @base
wright prune --untracked --latest --apply
wright list
```

## Terms

- **Plan**: the `plan.toml` blueprint for building one part
- **Part**: the built `.wright.tar.zst` part
- **Assembly**: a build-time grouping of plans
- **Inventory**: the local database of built parts on this machine
- **System**: the live machine being maintained

## Documentation

- [Getting Started](docs/getting-started.md)
- [Usage Guide](docs/usage.md)
- [Terminology](docs/terminology.md)
- [Writing Plans](docs/writing-plans.md)
- [Writing Assemblies](docs/writing-assemblies.md)
- [CLI Reference](docs/cli-reference.md)
- [Configuration](docs/configuration.md)
- [Architecture](docs/architecture.md)
- [Apply Design](docs/apply-design.md)
- [Dependencies](docs/dependencies.md)
- [Cookbook](docs/cookbook.md)

## License

MIT

# wright

A declarative, extensible, sandboxed Linux package manager for LFS-based distributions.

wright provides a complete package management and build system using TOML-based package descriptions, a lifecycle build pipeline with pluggable executors, namespace-isolated builds via bubblewrap, and transactional install/remove operations backed by SQLite.

## Features

- **Declarative TOML package descriptions** — define metadata, dependencies, sources, and build steps in a single `package.toml`
- **Lifecycle pipeline** — ordered build stages (prepare, configure, build, check, package) with pre/post hooks
- **Pluggable executors** — run build scripts with bash, python, lua, or custom runtimes
- **Sandbox isolation** — builds run inside Linux namespace containers via bubblewrap (bwrap)
- **Transactional operations** — atomic package install/remove with rollback support
- **Binary packages** — distributable `.wright.tar.zst` archives with embedded metadata

## Building

Requires Rust (stable) and Cargo:

```
cargo build --release
```

This produces three binaries in `target/release/`:

| Binary | Purpose |
|--------|---------|
| `wright` | Package manager (install, remove, query, verify) |
| `wright-build` | Build tool (parse package.toml, execute builds, create archives) |
| `wright-repo` | Repository tool (generate index from built packages) |

## Quick Start

Build a package from a hold directory:

```
wright-build /var/hold/extra/hello
```

Install the resulting archive:

```
wright install hello-1.0.0-1-x86_64.wright.tar.zst
```

Query installed packages:

```
wright list
wright query hello
wright files hello
```

Remove a package:

```
wright remove hello
```

## Documentation

See the [docs/](docs/) directory for detailed documentation:

- [Getting Started](docs/getting-started.md) — prerequisites, building, first package
- [Usage Guide](docs/usage.md) — full workflow: install Rust, compile Wright, write plans, build & install packages
- [Writing Plans](docs/writing-plans.md) — complete `package.toml` reference for plan authors
- [CLI Reference](docs/cli-reference.md) — complete command reference for all binaries
- [Configuration](docs/configuration.md) — wright.toml, repos.toml, executor definitions
- [Package Format](docs/package-format.md) — writing package.toml files
- [Architecture](docs/architecture.md) — code structure and module overview
- [Contributing](docs/contributing.md) — development setup, testing, conventions
- [Design Specification](docs/design-spec.md) — full technical design document

## License

MIT

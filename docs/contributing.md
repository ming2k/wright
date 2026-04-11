# Contributing

## Setup

```
git clone <repo-url>
cd wright
cargo build
cargo test
```

## Project Layout

```
src/
├── bin/wright.rs   # CLI entry point
├── lib.rs          # Library root
├── cli/            # clap schemas grouped by subcommand
├── commands/       # command handlers grouped by subcommand
├── config.rs       # Configuration
├── part/           # Part archive and version handling
├── builder/        # Build pipeline, executors, variables
├── database/       # SQLite layer
├── inventory/      # Local archive inventory and resolver
├── transaction/    # Install/remove with rollback
├── dockyard/       # Dockyard isolation
├── query/          # Read-only system inspection
└── util/           # Download, checksum, compression
tests/
├── integration.rs  # Integration test entry point
└── fixtures/       # Test plan data
```

See [architecture.md](architecture.md) for module details.

## Conventions

- `anyhow::Result` for binaries, `thiserror`-based `WrightError` for library code
- `tracing` macros for logging, not `println!` (except CLI output)
- Any CLI/config/feature changes must update the relevant docs
- Run `cargo fmt` and `cargo clippy` before committing

## PR Process

1. Feature branch from `main`
2. `cargo test && cargo clippy && cargo fmt --check` must pass
3. PR with description of changes and motivation

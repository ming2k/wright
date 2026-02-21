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
├── config.rs       # Configuration
├── package/        # Plan parsing, versioning, archive creation
├── builder/        # Build pipeline, executors, variables
├── database/       # SQLite layer
├── transaction/    # Install/remove with rollback
├── dockyard/       # Dockyard isolation (bubblewrap + native)
├── repo/           # Repository index, sync, source resolution
└── util/           # Download, checksum, compression
tests/
├── integration/    # End-to-end tests
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

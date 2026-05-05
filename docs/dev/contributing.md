# Contributing

## Setup

```bash
git clone <repo-url>
cd wright
cargo build
cargo test
```

## Project Layout

```
src/
├── bin/wright.rs     # CLI entry point
├── lib.rs            # Library root
├── cli/              # clap argument schemas grouped by subcommand
├── commands/         # command handlers grouped by subcommand
├── config.rs         # global configuration
├── builder/          # build orchestration and lifecycle execution
├── database/         # SQLite state layer and migrations
├── isolation/        # sandbox isolation (bubblewrap, sysroot)
├── part/             # archive format, local part store, pruning, version parsing, FHS validation
├── plan/             # plan discovery, manifest parsing, and validation
├── query/            # read-only system inspection and tree queries
├── transaction/      # install / upgrade / remove with rollback journal
└── util/             # helpers: download, checksum, compression, locking
tests/
├── integration.rs    # integration test entry point
└── fixtures/         # test plan data
```

See [Architecture](../explanation/architecture.md) and [Module Layout](module-layout.md) for details.

## Conventions

- `anyhow::Result` for binaries; `thiserror`-based `WrightError` for library code
- `tracing` macros for logging, not `println!` (except for intentional CLI output)
- Any CLI, config, or feature changes must update the relevant documentation
- Run `cargo fmt` and `cargo clippy` before committing

## PR Process

1. Branch from `main`
2. `cargo test && cargo clippy && cargo fmt --check` must pass
3. PR description should state what changed and why

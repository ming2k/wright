# Contributing

## Development Setup

### Prerequisites

- Rust stable toolchain (install via [rustup](https://rustup.rs/))
- Git

### Clone and Build

```
git clone <repo-url>
cd wright
cargo build
```

### Run Tests

```
cargo test
```

This runs both unit tests (embedded in source files) and integration tests (in `tests/`).

Run a specific test:

```
cargo test test_parse_hello_fixture
```

Run with output visible:

```
cargo test -- --nocapture
```

### Run Lints

```
cargo clippy
```

### Format Code

```
cargo fmt
```

## Project Layout

```
wright/
├── Cargo.toml          # Package manifest and dependencies
├── src/
│   ├── bin/            # Binary entry points (wright, wright-build, wright-repo)
│   ├── lib.rs          # Library root
│   ├── config.rs       # Configuration loading
│   ├── error.rs        # Error types
│   ├── package/        # Package parsing, version comparison, archive creation
│   ├── builder/        # Build pipeline, executors, variable substitution
│   ├── database/       # SQLite database layer
│   ├── transaction/    # Install/remove transactions with rollback
│   ├── resolver/       # Dependency graph and topological sort
│   ├── sandbox/        # Bubblewrap sandbox generation
│   ├── repo/           # Repository index, sync, source resolution
│   └── util/           # Download, checksum, compression utilities
├── tests/
│   ├── integration.rs  # Integration test entry point
│   ├── integration/    # Integration test modules
│   └── fixtures/       # Test package data
└── docs/               # Documentation
```

See [architecture.md](architecture.md) for detailed module descriptions.

## Coding Conventions

- **Documentation:** Any changes to features, CLI flags, or configuration must be accompanied by updates to the relevant documentation in `docs/` and `README.md`. Examples in documentation should be tested or manually verified.
- Use `anyhow::Result` for binary/application code, `thiserror`-based `WrightError` for library code
- All public functions in the library should return `crate::error::Result<T>`
- Use `tracing` macros (`info!`, `debug!`, `warn!`, `error!`) for logging, not `println!` (except in CLI output)
- Follow standard Rust naming: `snake_case` for functions/variables, `CamelCase` for types
- Run `cargo fmt` before committing
- Run `cargo clippy` and address warnings

## Testing

### Unit Tests

Unit tests live alongside the code they test, in `#[cfg(test)] mod tests` blocks. Focus areas:

- **Manifest parsing** (`src/package/manifest.rs`) — valid/invalid TOML inputs
- **Version comparison** (`src/package/version.rs`) — semver edge cases
- **Variable substitution** (`src/builder/variables.rs`) — all variables expand correctly
- **Dependency resolution** (`src/resolver/`) — cycle detection, topological sort

### Integration Tests

Integration tests are in `tests/` and use fixture data from `tests/fixtures/`. They test end-to-end flows:

- Building a package from a fixture
- Installing and removing packages
- Database operations
- File integrity verification

### Test Fixtures

Test packages live in `tests/fixtures/`. Each fixture is a directory with a `package.toml` and any supporting files.

## Pull Request Process

1. Create a feature branch from `main`
2. Make your changes with clear, focused commits
3. Ensure `cargo test`, `cargo clippy`, and `cargo fmt --check` all pass
4. Open a pull request with a description of the changes and motivation

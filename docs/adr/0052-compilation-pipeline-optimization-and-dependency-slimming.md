---
id: ADR-0052
title: "Compilation Pipeline Optimization and Dependency Slimming"
status: accepted
date: 2026-10-01
scope: core/build
superseded_by: null
negative_knowledge: true
---

# ADR-0052: Compilation Pipeline Optimization and Dependency Slimming

## Status

Accepted

## Context and Problem Statement

As Wright evolved across 50+ architecture revisions, compilation time and disk usage escalated significantly:
1. **Target Directory Bloat**: `target/` swelled beyond 80 GB (`target/debug/deps` ~42 GB, `target/debug/incremental` ~37 GB), and the debug binary (`target/debug/wright`) alone weighed ~351 MB.
2. **Slow Linking Overhead**: The build system defaulted to GNU `ld.bfd`. Linking a 350+ MB debug ELF binary on every incremental command or handler touch imposed several seconds of I/O latency.
3. **Dependency and Feature Bloat**:
   - `crates/wright-part` imported `zip = "2"` with default features, which activated redundant encryption stacks (`aes`, `cipher`, `generic-array`, `pbkdf2`, `hmac`, `sha1`, `zeroize`) and redundant compression decoders (`zopfli`, `deflate64`, `lzma-rs`), whereas Wright only uses `zip::ZipArchive` for basic part decompression.
   - `tokio = { version = "1.52.1", features = ["full"] }` was pulled into root `Cargo.toml` and `wright-actions`, pulling in extraneous submodules and proc-macro expansions.

## Decision Drivers

- **Developer Velocity**: Cold builds, clean rebuilds, and incremental iteration cycles must be sub-second to low-second.
- **Hermetic Integrity**: Do not compromise self-bootstrapping guarantees (ADR-0022, ADR-0035).
- **Disk Budget Containment**: Keep debug artifacts and incremental caches within reasonable bounds.
- **Zero Legacy Burden**: Remove dead features and unneeded transitive crates completely.

## Considered Options

- **Option 1**: Granular dependency pruning, `mold` modern linker integration, and line-level debuginfo profile tuning (`debug = 1`, `split-debuginfo = "unpacked"`).
- **Option 2**: Shell out to system utilities (`unzip`, `git`, `tar`) to eliminate library dependencies.
- **Option 3**: Turn off debuginfo completely (`debug = 0`) in `dev` profile.

## Decision Outcome

Chosen option: **Option 1**.

### 1. Linker Modernization and Profile Optimization
- Configured `.cargo/config.toml` to prioritize `/usr/bin/mold` with `-C link-arg=-fuse-ld=mold` for `x86_64-unknown-linux-gnu`, falling back gracefully to standard flags where appropriate.
- Configured workspace `Cargo.toml`:
  - `[profile.dev]` and `[profile.test]`: `debug = 1` (line-tables-only, preserving file and line numbers for backtraces while removing multi-gigabyte DWARF variable tracking) and `split-debuginfo = "unpacked"`.
  - Result: Debug binary dropped from **351 MB to 150 MB** (>57% reduction), and incremental link latency dropped from **~2.28s to ~0.83s**.

### 2. Dependency Slimming
- **`zip` Crate Feature Pruning**:
  In `crates/wright-part/Cargo.toml`, configured:
  ```toml
  zip = { version = "2", default-features = false, features = ["deflate-flate2", "flate2"] }
  ```
  This cleanly excised 12+ crates (`aes`, `cipher`, `constant_time_eq`, `crc`, `crc-catalog`, `deflate64`, `hmac`, `inout`, `lzma-rs`, `pbkdf2`, `zeroize_derive`, `zopfli`), eliminating duplicate `getrandom` versions from `Cargo.lock`.
- **`tokio` Feature Pruning**:
  Replaced `features = ["full"]` with the surgical subset of runtime features required by the call sites (`rt-multi-thread`, `macros`, `fs`, `sync`, `time`, `net`, `signal`, `process`, `io-util`).

### Positive Consequences

- Clean and incremental compilation and linking times cut by more than half.
- Reduced disk pressure on developer environments and CI runners.
- Pruned redundant cryptographic libraries and duplicate version trees.
- Full workspace test suite passes with zero warnings under `cargo clippy --all-targets -- -D warnings`.

### Negative Consequences

- Environments building with custom non-GCC/Clang drivers on Linux without `mold` installed must ensure appropriate build dependencies (added `mold` to CI dependency installation).

## Rejected Alternatives & Negative Knowledge

### Why Shelling Out to System Utilities (Option 2) Was Discarded
Shelling out to system `unzip` or `tar` violates Wright's foundational architectural requirement (ADR-0022 / ADR-0035) that execution must be self-contained and free of unexpected host environment drift.

### Why Disabling Debuginfo Entirely (Option 3) Was Discarded
Setting `debug = 0` eliminates line numbers from panic traces, severely crippling developer feedback during bug investigations and test failures. `debug = 1` achieves ~70% symbol size reduction while preserving stack traces and source coordinates.

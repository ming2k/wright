---
id: ADR-0042
title: "Single-Writer Actor and Concurrent Readers via rusqlite"
status: accepted
date: 2026-09-17
---

# ADR-0042: Single-Writer Actor and Concurrent Readers via rusqlite

## Status

Accepted

## Context

Wright's persistent state engine (`crates/wright-state`) previously relied on `sqlx` (`sqlx::SqlitePool`) to manage the system SQLite database (`wright.db`). While `sqlx` offered an `async/await` interface compatible with Tokio, its architecture introduced several fundamental mismatches with SQLite's embedded engine:

1. **Synthetic Asynchrony & Hidden Thread Handoff**: SQLite is an in-process, synchronous C library that executes filesystem syscalls directly. `sqlx` achieves "asynchronous" execution by delegating calls across internal channels to background worker threads. For microsecond-scale read queries, this cross-thread context switching overhead exceeded the query runtime itself.
2. **Pool-Induced Write Contention (`SQLITE_BUSY`)**: SQLite in WAL mode strictly enforces single-writer semantics. When an asynchronous connection pool issues write transactions from multiple concurrent futures, connections contend on SQLite's exclusive write lock, creating `SQLITE_BUSY` errors and requiring fragile retry/timeout configurations.
3. **Heavyweight Dependency Footprint**: `sqlx` with macro evaluation, compile-time query infrastructure, and network transport runtime abstractions imposed a heavy burden on build times and CLI binary footprint.
4. **Coarse-Grained Process Locking**: Because `sqlx` hid connection lifetimes behind a pool, `InstalledDb::open` was guarded by a monolithic, exclusive process lock, which prematurely disabled WAL's innate concurrent reader capabilities.

## Decision

We replace `sqlx` entirely with `rusqlite` coupled to a **Single-Writer Actor with Concurrent Readers** architecture:

1. **Dedicated Writer Actor OS Thread**:
   - A single, dedicated OS worker thread owns the persistent SQLite read-write connection for the entire application lifetime.
   - All mutation operations (`INSERT`, `UPDATE`, `DELETE`, and multi-step transactions) are serialized into an in-memory queue (`tokio::sync::mpsc::channel`) and executed synchronously on the writer thread, eliminating write lock contention and `SQLITE_BUSY` errors by construction.
2. **Concurrent Multi-Reader Surface**:
   - Read-only operations (`SELECT`, dependency resolution, file verification) bypass the writer queue entirely.
   - Readers access SQLite concurrently using read-only connections (`SQLITE_OPEN_READ_ONLY` with `PRAGMA query_only = ON`), bounded by a Tokio semaphore to prevent file-descriptor exhaustion.
   - Under SQLite WAL mode, readers never wait on the writer and the writer never waits on readers.
3. **Calibrated Zero-Churn WAL PRAGMAs**:
   - Every connection enforces hardware-conscious settings:
     - `PRAGMA journal_mode = WAL;`
     - `PRAGMA busy_timeout = 5000;`
     - `PRAGMA foreign_keys = ON;`
     - `PRAGMA synchronous = NORMAL;` (durable across OS crashes without disk stalls on every commit)
     - `PRAGMA journal_size_limit = 16777216;` (recycles WAL into a 16MB ring buffer, avoiding filesystem inode and allocation churn)
     - `PRAGMA wal_autocheckpoint = 1000;` (smooth ~4MB checkpoints)
     - `PRAGMA temp_store = MEMORY;`
4. **Embedded Deterministic Migration Runner**:
   - All schema migrations (001 through 020) are compiled into the binary as a constant sequence.
   - Migrations execute inside an immediate transaction, tracked via standard SQLite `PRAGMA user_version` while maintaining backward compatibility with legacy `_sqlx_migrations` metadata.

## Alternatives

### Retain sqlx with a single connection pool (`max_connections = 1`)

Configuring `SqlitePool` with a single connection serializes writes, but also forces all read queries into the same queue, completely breaking SQLite WAL's multi-reader capability and retaining `sqlx`'s compile-time and runtime overhead. Rejected.

### Unsynchronized rusqlite with `spawn_blocking`

Calling `rusqlite::Connection` directly inside `tokio::task::spawn_blocking` at every call site distributes writes across random threads in Tokio's blocking pool, directly re-introducing write lock contention and connection churn. Rejected.

## Consequences

- **Zero Write Contention**: Write transactions queue deterministically in memory; `SQLITE_BUSY` is eliminated.
- **Maximized Read Concurrency**: Read operations execute concurrently alongside writes under WAL snapshot isolation without blocking Tokio workers.
- **Minimal Dependencies & Fast Builds**: Removing `sqlx` removes large macro dependencies, significantly speeding up build times and reducing final binary size.
- **Hardware Lifespan**: Ring-buffered WAL recycling prevents flash wear and filesystem inode thrashing during high-volume package operations.
- **Clear Maintenance Model**: Schema migrations and query bindings are plain, explicit Rust code without proc-macro obscurity.

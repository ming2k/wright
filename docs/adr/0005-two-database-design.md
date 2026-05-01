# ADR-0005: Two-Database Design

## Status

Accepted

## Context

Wright needs to track two distinct states:

1. **Installed system state**: What is currently on the live filesystem.
2. **Local archive inventory**: Built `.wright.tar.zst` files available for reuse.

Options:

1. Single database with separate tables.
2. Two independent databases.

## Decision

Use two distinct SQLite databases:

- `installed.db` — authoritative state of the live system.
- `archives.db` — catalogue of locally built archives.

## Consequences

- **Performance**: The resolver can compute complex build and install plans without being slowed by thousands of file-level records in the installed database.
- **Resilience**: The `installed.db` must be kept extremely consistent. Separating the transient local build inventory reduces the risk of corruption during heavy build/prune cycles affecting the system's ability to boot.
- **Portability**: `archives.db` can theoretically be shared or synced as a repository index, whereas `installed.db` is unique to each machine.

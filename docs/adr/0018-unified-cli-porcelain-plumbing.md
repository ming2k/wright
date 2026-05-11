# ADR-0018: Unified CLI with Porcelain–Plumbing Separation

## Status

Accepted

## Context

Wright's command-line interface has grown organically. Commands are currently
defined in `src/cli/system.rs` (330+ lines mixing system-state and read-only
operations), `src/cli/build.rs`, `src/cli/launch.rs`, and `src/cli/prune.rs`.
The top-level `src/cli/mod.rs` flattens a "System" subcommand enum together
with standalone top-level variants. This creates several problems:

1. **No visual grouping in `--help`**: `wright --help` dumps every subcommand
   into a flat list. Users cannot tell at a glance which commands mutate the
   system, which are read-only queries, and which are maintainer-facing build
   tools.
2. **Scattered definitions and handlers**: CLI argument structs live in
   `src/cli/`, but command handlers live in `src/commands/` with a parallel
   directory hierarchy. The mapping is ad-hoc rather than convergent.
3. **Risk of tool proliferation**: As the feature set expands there is natural
   pressure to add separate binaries (`wright-build`, `wright-query`, …).
   Debian's experience with `dpkg-*`, `apt-*`, and `aptitude-*` shows that
   fragmented tool families hurt discoverability, increase documentation burden,
   and make shell completion harder. We want one entry point.

We need a structural refactor that:
- Presents a single `wright` binary (Unified CLI).
- Groups commands by user intent in `--help` output.
- Separates user-friendly high-level commands (porcelain) from low-level
  scripting primitives (plumbing).
- Makes the source-tree layout converge with the help groupings so that a
  contributor can locate code by reading `--help`.

## Decision

### 1. Unified CLI: one binary, no `wright-*` satellites

All functionality is exposed through the `wright` binary only. We will not
introduce helper binaries such as `wright-build`, `wright-query`, or
`wright-doctor`. Internally the binary may call into shared libraries, but the
user-visible surface is a single command tree.

Rationale: Debian's split between `dpkg`, `apt-get`, `apt-cache`, and
`aptitude` forced users to learn four overlapping vocabularies. Git's success
with `git <verb>` (one binary, many subcommands) demonstrates that a unified
CLI scales better, especially with good grouping and shell completion.

### 2. Command groups via clap `help_heading`

We use clap's `help_heading` attribute to partition subcommands into four
categories that match user intent:

| Heading | Intent | Examples |
|---------|--------|----------|
| **System Management** | Mutate live system state | `install`, `remove`, `upgrade`, `merge` |
| **Query & Inspection** | Read-only introspection | `list`, `files`, `check`, `doctor`, `history` |
| **Build & Packaging** | Forge, lint, and bootstrap | `build`, `lint`, `launch` |
| **Cache & Maintenance** | House-keeping and cleanup | `prune` |

Each heading is declared on its subcommand enum so that `wright --help` prints
the groups explicitly:

```
Usage: wright <COMMAND> [OPTIONS]

System Management:
  install    Install plans into the live system
  remove     Remove deployed parts
  ...

Query & Inspection:
  list       List installed parts
  files      List files owned by a part
  ...
```

### 3. Porcelain vs Plumbing distinction

Within the unified CLI we distinguish two layers **by naming and documentation**,
not by separate binaries:

- **Porcelain** commands are the user-facing defaults. They are safe to run
  interactively, produce human-readable output, and may combine several
  lower-level steps. Examples: `wright install`, `wright upgrade`,
  `wright doctor`.
- **Plumbing** commands are low-level primitives intended for scripting,
  piping, and CI. They do one thing, produce machine-parseable output by
  default, and carry fewer guard-rails. Examples: `wright merge` (direct
  archive deployment), `wright build` (compile without auto-deploy),
  `wright list` (plain newline-separated names suitable for `xargs`).

This distinction is documented in the CLI reference; clap groupings do not
enforce it mechanically.

### 4. Convergent file layout

The source tree is reorganised so that **file paths mirror help headings**.

```
src/cli/
  mod.rs              # Top-level Cli struct and Commands enum (groups only)
  system.rs           # System Management subcommands + args
  query.rs            # Query & Inspection subcommands + args
  build.rs            # Build & Packaging subcommands + args
  maintenance.rs      # Cache & Maintenance subcommands + args
  common.rs           # Shared ValueEnum types

src/commands/
  mod.rs              # Dispatch router
  system.rs           # Dispatcher for System Management handlers
  query.rs            # Dispatcher for Query & Inspection handlers
  build.rs            # Dispatcher for Build & Packaging handlers
  maintenance.rs      # Dispatcher for Cache & Maintenance handlers
  handlers/
    system/           # Impl files for install, remove, upgrade, merge, assume…
    query/            # Impl files for list, files, check, doctor, history
    build/            # Impl files for build, lint, launch
    maintenance/      # Impl files for prune
```

Rules:
- `src/cli/<group>.rs` owns **argument definitions** for that group.
- `src/commands/<group>.rs` owns the **dispatch logic** for that group.
- `src/commands/handlers/<group>/` owns the **implementation details**.
- The dispatch module matches on the CLI enum and calls the handler; it does
  not contain business logic.

This convergence means: if a user sees `wright doctor` under "Query &
Inspection" in `--help`, the argument struct is in `src/cli/query.rs` and the
handler is reachable through `src/commands/query.rs`.

### 5. What we keep and what we move

Existing commands are retained without behaviour changes; only their
organisation moves:

| Command | Old location | New CLI group | New dispatch |
|---------|-------------|---------------|--------------|
| `merge` | `cli/system.rs` | System Management | `commands/system.rs` |
| `install` | `cli/system.rs` | System Management | `commands/system.rs` |
| `upgrade` | `cli/system.rs` | System Management | `commands/system.rs` |
| `remove` | `cli/system.rs` | System Management | `commands/system.rs` |
| `assume` | `cli/system.rs` | System Management | `commands/system.rs` |
| `unassume` | `cli/system.rs` | System Management | `commands/system.rs` |
| `list` | `cli/system.rs` | Query & Inspection | `commands/query.rs` |
| `files` | `cli/system.rs` | Query & Inspection | `commands/query.rs` |
| `check` | `cli/system.rs` | Query & Inspection | `commands/query.rs` |
| `doctor` | `cli/system.rs` | Query & Inspection | `commands/query.rs` |
| `history` | `cli/system.rs` | Query & Inspection | `commands/query.rs` |
| `build` | `cli/build.rs` | Build & Packaging | `commands/build.rs` |
| `lint` | `cli/mod.rs` | Build & Packaging | `commands/build.rs` |
| `launch` | `cli/launch.rs` | Build & Packaging | `commands/build.rs` |
| `prune` | `cli/prune.rs` | Cache & Maintenance | `commands/maintenance.rs` |

## Consequences

- `--help` output becomes self-documenting. Users immediately see which
  commands are safe read-only queries and which mutate system state.
- There is a single binary to install, complete, and document.
- Contributors can locate command code by following the group name from
  `--help` into `src/cli/<group>.rs` and `src/commands/<group>.rs`.
- The refactor is mechanical (moving code, no logic changes). Existing tests
  continue to pass because CLI flag names and semantics are unchanged.
- Future commands are added by choosing a group, creating the args struct in
  the corresponding `src/cli/<group>.rs`, and adding the handler path through
  the matching `src/commands/<group>.rs`.
- The porcelain/plumbing distinction is social/documentation-based rather than
  enforced by code. This is intentional: a power user should be able to pipe
  plumbing commands together without artificial barriers.

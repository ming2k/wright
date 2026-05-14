# ADR-0018: Unified CLI with Porcelain–Plumbing Separation

## Status

Accepted. The file-layout section (§4, §5) is **superseded by
[ADR-0020](0020-merge-cli-and-commands-directories.md)**, which merges
`src/cli/` and `src/commands/` into a single directory. All other rulings
in this ADR — one unified binary, command groups via `help_heading`, and
the porcelain/plumbing distinction — remain in force.

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

### 4. Convergent file layout (Flattened)

The source tree is now **fully flattened**.

```
src/cli/
  mod.rs              # Top-level Cli aggregator
  install.rs          # Install args
  remove.rs           # Remove args
  ...                 # Every command has its own .rs file

src/commands/
  mod.rs              # Dispatch router
  common.rs           # Shared dispatch helpers (locks, db resolution)
  install.rs          # Dispatcher for install
  remove.rs           # Dispatcher for remove
  ...                 # Every command has its own .rs file
```

Rules:
- `src/cli/<command>.rs` owns **argument definitions** for that command.
- `src/commands/<command>.rs` owns the **dispatch logic** for that command.

### 5. Updated Command Mapping

| Command | Args location | Dispatch location |
|---------|---------------|-------------------|
| `merge` | `cli/merge.rs` | `commands/merge.rs` |
| `install` | `cli/install.rs` | `commands/install.rs` |
| `upgrade` | `cli/upgrade.rs` | `commands/upgrade.rs` |
| `remove` | `cli/remove.rs` | `commands/remove.rs` |
| `provide` | `cli/provide.rs` | `commands/provide.rs` |
| `list` | `cli/list.rs` | `commands/list.rs` |
| `files` | `cli/files.rs` | `commands/files.rs` |
| `check` | `cli/check.rs` | `commands/check.rs` |
| `doctor` | `cli/doctor.rs` | `commands/doctor.rs` |
| `history` | `cli/history.rs` | `commands/history.rs` |
| `build` | `cli/build.rs` | `commands/build.rs` |
| `lint` | `cli/lint.rs` | `commands/lint.rs` |
| `launch` | `cli/launch.rs` | `commands/launch.rs` |

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
s/query.rs` |
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

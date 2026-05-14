# ADR-0020: Merge `src/cli/` and `src/commands/` into a single directory

## Status

Accepted. Partially supersedes [ADR-0018](0018-unified-cli-porcelain-plumbing.md)
(file-layout section only — the unified-binary, group-by-intent, and
porcelain/plumbing decisions remain in force).

## Context

ADR-0018 split each command across two parallel files:

- `src/cli/<cmd>.rs` — clap `Args` struct (arguments and help text).
- `src/commands/<cmd>.rs` — dispatch glue: extract `root`, resolve `db_path`,
  acquire the process lock, build a request, call `operations::*`.

In practice the dispatch file averaged ~15 lines per command. Most of it was
boilerplate that translated `Args` fields into the matching
`operations::execute_*` call — for example, mapping `DomainArg` to
`DependentsMode` or `MatchPolicyArg` to `MatchPolicy`. Two problems emerged:

1. **Naming collision.** `src/cli/install.rs` and `src/commands/install.rs`
   describe the same command at the same conceptual level. The directory
   names ("cli" and "commands") both gesture at "the command-line surface,"
   so a new contributor opening `install.rs` does not know which one they
   want. Every command had this collision (`build`, `check`, `install`,
   `launch`, …).
2. **Indirection without payoff.** The split predicted a separation of
   concerns that does not actually hold. Changes to an `Args` struct and its
   dispatch arrive together in nearly every PR; splitting them across two
   files adds a cross-import (`use crate::cli::install::InstallArgs`) and a
   second editor tab without isolating anything.

The widely-adopted Rust CLI convention (cargo, rustup, uv, ripgrep) is one
module per command that owns both the clap `Args` and its handler, with
business logic in a separate lower-level layer. Wright already has that lower
layer (`src/operations/`), so the middle layer (`src/commands/`) was
redundant.

## Decision

### 1. One module per command

Each command lives in a single file `src/cli/<cmd>.rs` that owns its `Args`
struct **and** its handler:

```rust
#[derive(clap::Args)]
pub struct InstallArgs { /* clap fields */ }

pub async fn run(args: InstallArgs, ctx: &Context<'_>) -> Result<()> {
    // setup, arg mapping, then a call into operations::*
}
```

`src/commands/` is deleted. `src/operations/` is unchanged.

### 2. A single `Context` carries per-invocation state

`src/cli/common.rs` defines:

```rust
pub struct Context<'a> {
    pub config: &'a GlobalConfig,
    pub db_path: PathBuf,
    pub root_dir: PathBuf,
    pub verbose: u8,
    pub quiet: bool,
}
```

with `open_db()` and `ensure_lock_and_part_store()` methods. The top-level
`dispatch` builds the `Context` once and passes `&Context` to every handler,
replacing the per-arm boilerplate that previously lived in `commands/mod.rs`.

Adding a new global flag (e.g. `--json`) requires changing only `Context`
and the relevant handlers, not every dispatch arm.

### 3. `build.rs` cfg gating preserves the merge

`build.rs` `#[path]`-includes `src/cli/mod.rs` to drive `clap_complete` and
`clap_mangen`. Including the merged tree directly would force `build.rs` to
compile handler bodies that reference `crate::operations`, `crate::resolve`,
etc. — modules that do not exist in the build-script crate.

We resolve this with a custom cfg `with_handlers`. `build.rs` emits

```
cargo::rustc-cfg=with_handlers
cargo::rustc-check-cfg=cfg(with_handlers)
```

which is set **only when Cargo compiles the main crate**, not when it
compiles `build.rs` itself. Handler functions, `Context`, and dispatch glue
in `src/cli/*.rs` carry `#[cfg(with_handlers)]`. The clap `Args` structs and
the top-level `Cli`/`Commands` enums are unconditional so `build.rs` can
still generate completions and man pages from them.

Cargo.toml declares the cfg for the linter:

```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(with_handlers)'] }
```

### 4. Cross-layer helpers move to `util/`

The stdin-reading helper used by `install` and `merge` was previously
`commands::install::collect_install_args` and was imported by
`operations::merge` (a wrong-direction dependency). It now lives in
`src/util/stdin.rs` as `collect_stdin_args` and is imported by both call
sites symmetrically.

## Consequences

- Contributors find a command's entire CLI surface — flags, help text, and
  handler — in one file.
- Eliminates the `cli/install.rs` ↔ `commands/install.rs` naming ambiguity.
- The `with_handlers` cfg is a technical artifact of how `build.rs`
  consumes the clap tree. It is invisible to anyone editing a single
  command file (just one attribute on `run` and on the handler-only
  imports) but contributors adding a new command must apply the same
  pattern.
- `operations::*` keeps its existing public API. Tests that previously
  invoked `commands::system::dispatch_install(...)` now construct a
  `Context` and call `cli::install::run(args, &ctx)`.
- This decision does not affect ADR-0018's other rulings: one binary, group
  by intent via `help_heading`, porcelain/plumbing distinction by
  documentation and naming.

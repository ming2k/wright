# ADR-0032: CLI Surface Consistency Conventions

## Status

Accepted

## Context

The CLI grew command by command, and the surface drifted: the same concept
had different spellings on different commands (`--dry-run` existed nowhere
and `--latest` was mandatory on `prune`; `remove -r` meant `--recursive`
while `resolve -r` meant `--rdeps`), some flags were effectively hidden
aliases (`--deps=forge`), `--root` existed on only some database-touching
commands, and failures escaped through a mix of error returns and mid-command
`process::exit` calls with different exit codes.

This drift costs every user relearning per-command dialects and makes
scripting fragile. The conventions below were applied across the whole
surface in one pass and are recorded here so future commands extend the
pattern instead of inventing new ones.

## Decision

### Dry-run spelling

State-changing commands preview with `-n`/`--dry-run`, which fully resolves
the operation, prints the plan, and returns before any side effect.
`wright prune` is the deliberate inversion: its default is already a dry run,
so it takes `--apply` to act. New prune-like commands (safe by default,
destructive only on request) follow the `--apply` pattern; everything else
uses `--dry-run`.

### Short flags

Short flags are allocated globally and stay unique across commands:

| Short | Long | Commands |
|-------|------|----------|
| `-f` | `--force` | build, install, upgrade, package, launch, merge, remove |
| `-n` | `--dry-run` | install, launch, remove, merge, upgrade |
| `-c` | `--clean` | build, install |
| `-d` | `--deps` | install, resolve |
| `-r` | `--rdeps` | install, resolve |
| `-t` | `--tree` | resolve |
| `-l` | `--long` | list |
| `-o` | `--orphans` | list |
| `-p` | `--print-parts` | package |

A letter is never reused for a different meaning on another command; an
option that cannot keep the shared letter stays long-only (hence
`remove --recursive` and `remove --cascade` have no shorts).

### `--root`

Every command that reads or writes a target root's database accepts
`--root <DIR>`: install, upgrade, remove, merge, provide, check, doctor,
list, files, owner, history, and launch (where it is required). Pure
build-side commands (resolve, build, package, lint, clean, prune) do not.

### `--json`

Query commands (`list`, `files`, `owner`, `history`, `check`) accept `--json`
and print a stable machine-readable shape; empty results print `[]` (`check`
prints a report object with an empty `issues` array). `wright check` keeps
its exit code (0 = clean, 1 = issues found) as the primary machine interface,
with `--json` supplying the structured detail. `wright owner` still exits 1
when any path is unowned, printing the JSON array first and the error report
on stderr.

### Errors and exit codes

No operation calls `process::exit` mid-command. Handlers return errors, and
the top level prints the cargo-style multi-line failure report on stderr and
exits 1. SIGINT exits 130. The binary restores the default SIGPIPE
disposition so piping into `head` dies quietly instead of panicking.

### Value names

Plan-ish positionals render as `TARGET`, part-ish ones as `PART`, `owner`'s
paths as `FILE`, and `provide`'s version positional as `VERSION`.

### Global options

`--config`, `--db`, `-v`/`--verbose`, and `--quiet` are true clap globals and
may appear before or after the subcommand. `-v` and `--quiet` conflict.
Verbosity maps: default `info`, `-v` = debug, `-vv` = trace, `--quiet` =
warnings and errors only.

## Alternatives

### Per-command short flags

Maximizes mnemonic freedom per command but forces users to relearn letters
per subcommand and produced exactly the collisions this ADR removes.
Rejected.

### `--dry-run` everywhere, including prune

Uniform, but it would force an explicit flag for prune's safe path, which is
the invocation that should be the default. A command whose only mode is
destructive earns the inverted spelling. Rejected.

### Keep mid-command `process::exit` for deep failures

It bypasses the failure report, skips cleanup, and produced exit-code
lottery (including 101 from SIGPIPE panics). A single propagation path is
testable and scriptable. Rejected.

### `--json` on every command

Machine output is a contract; promising it on state-changing commands would
freeze progress rendering and partial-failure shapes that are still
evolving. Query commands and exit codes cover the scripting cases. Rejected.

## Consequences

- New commands must fit the allocation tables above before adding a short
  flag, a `--root`, or a `--json`; anything that cannot fit stays long-only.
- Scripts can rely on: exit 1 plus a stderr failure report for any error,
  0/1 from `check`, 130 on interrupt, and quiet pipe behavior.
- The conventions are enforced by review; there is no lint that derives them
  from the clap schemas yet.

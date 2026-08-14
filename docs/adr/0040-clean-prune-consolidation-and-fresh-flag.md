# ADR-0040: Consolidate maintenance deletion into `clean` and rename the from-scratch forge flag to `--fresh`

## Status

Accepted

## Context

ADR-0032 unified the CLI surface, but two of its spellings carried an
ambiguity that surfaced in use.

The `-c`/`--clean` flag on `build` and `install` wipes the forge workspace
*before* building so the forge starts from scratch. Read on a command line,
however, `--clean` presents deletion as the command's action — or as cleanup
*after* the build, which is the meaning the spelling carries in other
toolchains (`makepkg --clean`). The collision with the standalone
`wright clean` command, whose purpose actually is deletion, made the flag
the second meaning of the same word on the same surface.

At the same time, `clean` and `prune` were two deletion verbs with
overlapping scope: both remove archives from the parts directory, but with
different retention rules (`--parts` deleted every archive of the named
plans; `prune` deleted only superseded versions) and opposite safety
defaults (`clean` deleted immediately; `prune` was a dry run until
`--apply`). The `--parts` spelling added its own misreading: it suggested
deleting deployed parts, which is `remove`'s job, when it actually deletes
built archive files.

## Decision

### `--fresh` spelling

The from-scratch forge flag is `-c`/`--fresh` on every command that
forges: `build`, `install`, `upgrade`, and `launch`. On `upgrade` and
`launch`, where `--force` previously implied the wipe, `--fresh` is the
independent spelling and `--force` keeps implying it. The short flag
keeps its allocation from ADR-0032; only the long name changes.
`--clean` remains as a hidden alias for one release, then is removed.
The standalone `wright clean` command keeps its name: deletion is its
actual purpose, so the word is precise there.

### Maintenance command consolidation

`wright clean` is the single disk-reclamation command, with scope flags
selecting what is deleted:

- default (no flags): build workspaces — all of them, or those of the
  named plans;
- `--archives`: also delete every built archive of the named plans;
- `--stale`: delete only superseded (non-latest) archive versions of each
  (plan, output) pair, skipping workspace cleanup;
- `--logs`: also delete command logs;
- `-n`/`--dry-run`: preview the exact removal set without deleting.

`--archives` replaces `--parts`, which stays as a hidden alias for one
release. `--stale` conflicts with `--archives`. `wright prune` is a hidden
deprecated alias for `wright clean --stale` for one release; unlike the
canonical spelling it stays a dry run without `--apply`, preserving its
historical default.

### Safety-model rule

The `prune`-style `--apply` inversion from ADR-0032 no longer exists.
Every deletion the consolidated command can perform targets rebuildable
artifacts (workspaces, archives, logs), so one safety model suffices:
state-changing commands execute by default and preview with
`-n`/`--dry-run`, without exception.

This ADR supersedes ADR-0032's short-flag row for `-c` and its dry-run
inversion sections. ADR-0032's remaining conventions stay in force.

## Alternatives

### Keep the split commands and only rename flags

`clean` and `prune` would still overlap on the parts directory and force
users to learn two verbs for one kind of task. Rejected.

### `--from-scratch` instead of `--fresh`

Maximally explicit, but long to type and unable to keep the globally
allocated `-c`. `--fresh` reads as a build mode rather than a deletion
action, which is the property the rename exists for. Rejected.

### Umbrella named `gc` or `reclaim`

`gc` is jargon inherited from Git and says nothing to users outside that
vocabulary; `reclaim` is accurate but unfamiliar as a command verb.
`clean` already means deletion on this surface and becomes unambiguous the
moment the flag stops using the word. Rejected.

### Make the consolidated command safe by default

Keeping dry-run-by-default for `--stale` inside `clean` would give one
command two safety models, and extending the inversion to all of `clean`
would make routine workspace cleanup two-step for no risk reduction:
everything the command deletes is rebuildable. Rejected.

## Consequences

- Scripts using `--clean` or `-c` on `build`/`install` keep working
  unchanged; scripts using the long spelling should migrate to `--fresh`
  before the alias is removed. `upgrade` and `launch` gain `-c`/`--fresh`
  (with the same `--clean` alias); their `--force` still implies the
  wipe, so existing invocations behave as before.
- Scripts using `wright prune` or `clean --parts` keep working unchanged
  for one release; both aliases warn or hide rather than fail.
- `clean --stale` executes immediately where `prune` defaulted to a dry
  run; the migration note in the changelog calls this out.
- The hidden `prune` alias and the `--clean`/`--parts` aliases are
  scheduled for removal one release after introduction.
- `docs/reference/cli-reference.md`, `docs/reference/build-mechanics.md`,
  and the first-steps tutorial carry the new spellings; ADR-0032's
  superseded rows are marked in the ADR index.

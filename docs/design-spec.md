# Current Design Summary

This document replaces the older historical spec. Wright is now a
source-first, local-first system with one primary CLI, distinct build/system
subcommands, and one local archive inventory.

## Core Objects

- `plan`: the source definition for one buildable unit
- `part`: a built `.wright.tar.zst` archive
- `assembly`: a named set of plans used as a build or apply target
- `system`: the installed live state tracked in `installed.db`
- `inventory`: the local catalog of built archives tracked in `archives.db`

There is no separate indexing/publish manager and no install-time grouping
model beyond assemblies.

## Tool Boundaries

- `wright build` builds parts from plans and records successful outputs in the
 local inventory
- `wright` installs, upgrades, removes, verifies, and applies those locally
 available parts to the live system

The main workflows are:

```bash
wright build curl
wright install curl

wright apply @base

wright resolve openssl --rdeps=all --depth=0 | wright build --force --print-parts | wright install
```

## Intended Workflow

Wright is optimized for self-hosted maintenance:

- plans are the source of truth
- built archives exist mainly for rollback, recovery, and local reuse
- `wright apply` is the preferred command when you want the system to match
 current plans or assemblies
- `wright prune` cleans stale or stray archives from the local store

## Data Layout

Typical paths:

```text
/etc/wright/wright.toml
/var/lib/wright/plans/
/var/lib/wright/assemblies/
/var/lib/wright/parts/
/var/lib/wright/state/installed.db
/var/lib/wright/state/archives.db
/var/lib/wright/lock/installed.db.lock
/var/lib/wright/lock/archives.db.lock
```

`installed.db` tracks installed system state. `archives.db` tracks built archives
available for reuse or installation.

## Design Constraints

- build and install are separate phases
- successful builds are registered automatically in the local inventory
- install and upgrade resolution uses only the local inventory
- assemblies are the only built-in grouping abstraction
- published binary distribution is out of scope for the default architecture

## No Magic Behavior

Wright does not perform implicit actions on behalf of the plan author. If the
tool does something, it must be because the plan explicitly asked for it.

Wright targets LFS-based distributions where the user base consists of power
users who understand what they are doing and expect predictable, auditable
behavior. Implicit automation that is convenient for casual users is a poor
trade-off here: it hides intent, makes plans harder to read, and introduces
edge cases that require even more implicit rules to handle.

**Concrete example — patch application.** A plan that needs to apply patches
declares them as `[[sources]]` entries (so they are fetched and verified like
any other source) and applies them explicitly in the `prepare` script:

```toml
[[sources]]
uri = "patches/fix-headers.patch"
sha256 = "SKIP"
```

```sh
# prepare
patch -p1 < "${WRIGHT_SRC_DIR}/fix-headers.patch"
```

Wright will never auto-detect `.patch` files and apply them silently. That
would hide the strip level, application order, and any conditional logic from
the reader. Two lines of shell are clearer and more flexible than any implicit
convention.

When evaluating a feature request, ask: does this save meaningful work, or
does it only save the user from writing something explicit and readable? If the
latter, prefer keeping behavior explicit.

For command details and current examples, use the rest of `docs/`.

# ADR-0041: File-Backed Ledger — Build Records, Snapshots, and `.BUILDINFO`

## Status

Accepted

Amends [ADR-0033](0033-plan-source-snapshots.md): its capture (`.PLANSRC`
member) and surface decisions stand; its persistence decision (the
`plan_snapshots` table) is replaced by section 2 below.

## Context

Wright's audit data answers three recurring operator questions:

- **"Will this installed part even run here?"** A part built on a newer
  microarchitecture (or a different kernel, a different machine) can fail
  after deployment in ways file-level integrity checks cannot explain. The
  archive carried no record of the host that produced it.
- **"What will the next upgrade of this plan cost?"** From-source
  maintenance means every upgrade is a compile. Estimating that cost needs
  history: per-stage and total durations, download and staging sizes, and
  the failure rate — none of which was persisted anywhere (the timing
  facility only printed its report).
- **"What exactly did the plan say when this was built?"** ADR-0033
  answered this with the `plan_snapshots` table. It works, but the content
  is a few kilobytes of TOML per row, and the only way to read it was a SQL
  query. Inspection with native tools (`cat`, `diff`, `grep`) — the way
  operators actually audit a machine — was needlessly indirect.

Two existing stances bound the answer. Plans are portable source and must
stay free of machine-local state ("share the plan, not the part"), so the
plan tree is not a candidate location. And audit data is advisory
(ADR-0016/0023): recording it must never fail a build, seal, or deploy.

## Decision

Machine-local audit data lives as plain files in a **ledger directory**
(`general.ledger_dir`, default `/var/lib/wright/ledger`), organized per
plan:

```text
<ledger>/<plan>/builds.jsonl                        one JSON record per build attempt
<ledger>/<plan>/snapshots/<yyyymmddThhmmssZ>-<sha256>.toml
<ledger>/_detached/<yyyymmddThhmmssZ>-<sha256>.toml  legacy rows whose plan is gone
```

1. **`.BUILDINFO` archive member.** Sealing embeds a TOML `.BUILDINFO`
   beside `.PLANSRC`: the sealing `wright_version` and the host's platform
   (`hostname`, `os`, `kernel`, `arch`, `cpu_model`, `cpu_cores`,
   `memory_bytes`, full `cpu_flags`). It is metadata, not payload — excluded from `.FILELIST`
   and deploy-time collection, optional by contract, and it travels with
   the archive because "what built this artifact?" is a question *about
   the artifact*. Deleting the archive deletes the record, which is
   correct: with no artifact there is nothing to diagnose.
2. **Snapshots move from the table to files.** Deploy/upgrade registration
   appends `<plan>/snapshots/<timestamp>-<checksum>.toml` only when that
   checksum is not already recorded for the plan — so `ls` reads as the
   plan's source history and `diff` answers drift with no tooling. Nothing
   is ever deleted. `plans.plan_checksum` stays in the database as the
   relational pointer drift detection joins on. `InstalledDb::open`
   exports any legacy `plan_snapshots` rows into the ledger *before* the
   dropping migration runs (orphaned rows land in `_detached/`); a failed
   export aborts the open rather than lose data.
3. **Build-cost records.** Every forge attempt — success or failure —
   appends one JSON line to `<plan>/builds.jsonl`: plan identity and
   `plan_checksum`, a host summary (flags stripped; `.BUILDINFO` carries
   those), per-step durations (`charge`, each forge stage that ran,
   `slice`) plus wall-clock total, cached-source bytes, staging-tree bytes
   and file count, and the flattened error on failure. Records from
   partial runs (`--stage`, `--fetch-only`, `--until-stage`) are marked
   `full: false` so cost estimates can filter them out.
4. **Advisory everywhere.** Ledger writes warn and are dropped on failure;
   reads degrade to "no record". No state-changing operation consults the
   ledger.

A database redirected by `--root`/`--db` keeps its ledger beside itself
(`<db dir>/ledger`) — the same derivation rule as locks and journals — so
a target root's audit data never mixes with the host's.

## Alternatives

**Keep snapshots in the database; answer cost questions with a new
table.** Rejected — the content is plain text a human wants to read and
diff directly; a SQL query (or a dedicated subcommand wrapping one) is
strictly more friction than `cat`/`diff`, and the cost history must
survive `prune` deleting the very archives it describes, which a
deployment-table row would not obviously do.

**Write build reports into the plan directory.** Rejected — plans are the
portable, shareable artifact; embedding machine-specific build facts would
leak one machine's timings into every copy of the plan and collide with
read-only or version-controlled plan trees.

**One report file per build under a reports directory.** Rejected — cost
estimation wants series data (recent N builds, averages), which a per-plan
append-only JSONL gives natively and scattered files do not.

**Embed build timing in `.BUILDINFO` as well.** Rejected — timing history
is a series, not a property of one artifact; the archive keeps only the
facts needed to diagnose that artifact (platform), while the series lives
in the ledger where deletion of old archives cannot destroy it.

## Consequences

### Positive

- "Installed but doesn't run" diagnosis gains hard data: CPU flags/model,
  kernel, and build host ride inside every archive.
- Upgrade cost becomes a query over local history
  (`tail /var/lib/wright/ledger/<plan>/builds.jsonl | jq`), including
  failure rate — with no new CLI surface to learn.
- Plan-source snapshots are inspectable and diffable with native tools,
  and the plan's source history accumulates automatically.
- The database shrinks to relational state; text payloads live in the
  filesystem, which is what filesystems are for.

### Negative

- Ledger writes are no longer transactional with deploy registration; a
  crash between them can lose a snapshot line (the archive's `.PLANSRC`
  remains the backup copy, and re-deploying re-records it).
- The ledger grows monotonically; retention/pruning policy is deliberately
  deferred.
- Two storage layers (DB pointer + file content) must stay consistent by
  convention: drift detection trusts `plans.plan_checksum` and degrades to
  the checksum line when the file is absent.

### Migration

- `plan_snapshots` is dropped by migration V20 after `InstalledDb::open`
  exports its rows; installs that never recorded provenance are
  unaffected.
- `wright doctor` and `wright plan` read from the ledger; behavior is
  unchanged apart from the storage backend.

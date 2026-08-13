# ADR-0038: Git Sources Cached as Tree Snapshots

## Status

Accepted

## Context

Since ADR-0022, git sources were cached as bare repositories under
`source_dir/git/` and materialized by a local fetch from that cache into the
build workspace during the extract stage. The design kept recurring costs:

- **Repository conventions leak into the build workspace.** A mirror fetch
  into a non-bare repository updates `refs/heads/*`, which writes reflog
  entries; reflog entries require a committer identity. Under `sudo` there
  is no configured identity, so the fetch aborted its ref transaction after
  the pack had already been received, and the error surfaced only as
  "Failed to update references to their new position to match their remote
  locations".
- **Incremental updates of shallow caches were fragile.** Updating a cached
  shallow fetch spuriously failed negotiation in ways that forced a
  stale-cache detection and re-clone retry path (ADR-0035).
- **The cache was not a source artifact.** A bare repository is opaque
  (not checksum-verifiable, not directly inspectable) and unbounded for
  commit-hash pins, which mirrored the repository's entire history — the
  source cache's contract is to hold the pristine, complete source.
- Some builds legitimately need git metadata in the work tree, e.g. plans
  whose prepare stage runs `git submodule update --init`.

## Decision

**Git sources are cached as tree snapshot tarballs directly under
`source_dir/`; the bare-repository cache is gone.**

- Fetching is minimal-effort: a shallow fetch of exactly the requested ref.
  A 40-character commit hash first attempts a shallow fetch by hash and
  falls back to a full mirror fetch on servers that refuse it.
- After a successful fetch, the resolved tree is written to
  `<source_dir>/<repo>-<ref>-<urlhash8>.tar.zst` — plain file entries only,
  no git metadata; submodule gitlinks carry no content and are skipped.
  Entry mtimes are zeroed, so the same commit always produces the same
  snapshot bytes.
- The extract stage unpacks the snapshot like any other archive. No git
  operations happen on the extract path for snapshot sources.
- A source declared with `git_metadata = true` keeps repository semantics:
  it is cloned from upstream straight into the work directory (shallow for
  named refs, a full mirror for commit-hash refs). Such sources bypass the
  source cache and always fetch from the network.
- The stale-cache refresh machinery from ADR-0035 is deleted. Snapshot
  fetches always clone into a fresh temporary repository and publish the
  tarball by atomic rename, so there is no incremental cache state that
  could go stale.

The invariants this decision preserves: all git operations still go through
`gix` (ADR-0035); Wright never shells out to a system `git` (ADR-0022).

## Alternatives

### Keep the bare-repository cache

The cache as a git database only pays off when many refs of one repository
are fetched over time and share objects. Plan sources overwhelmingly pin
one ref per repository, so the dedup rarely materializes, while the extract
path keeps importing repository conventions (reflogs, identities) into what
should be a plain file copy. Rejected.

### Non-bare cache repositories

A worktree in the cache can only represent one ref, duplicates storage, and
inherits exactly the non-bare conventions — reflog writes, committer
identity — that caused the motivating incident. Rejected.

### Materialize submodules into the snapshot

Recording gitlink targets and silently fetching submodule content during
extraction would hide network operations inside an otherwise declarative
source list. Plans that need submodules declare `git_metadata = true` and
drive submodule fetching explicitly in their prepare scripts, consistent
with ADR-0004 (no magic behavior). Rejected.

## Consequences

### Positive

- The source cache holds only source artifacts: inspectable tarballs whose
  bytes are deterministic for a given commit.
- Extract is an untar for every source type: no ref transactions, no
  reflogs, no identity requirements on the extract path.
- Cache size is bounded at one snapshot per pinned ref; full-history
  mirrors no longer persist.
- The stale-cache detection and re-clone retry path is gone.

### Negative / Tradeoffs

- Different refs of one repository no longer share objects; each pinned
  ref costs one full snapshot.
- `git_metadata = true` sources fetch from the network on every build.
- Existing bare-repository caches under `source_dir/git/` are orphaned and
  can be deleted manually.

## References

- [ADR-0035](0035-git-fetch-via-gitoxide.md) — git via gitoxide; its cache
  layout and extract mechanics are superseded by this ADR.
- [ADR-0022](0022-git-fetch-via-libgit2-no-system-git.md) — the
  no-system-`git` invariant, retained.
- [ADR-0004](0004-no-magic-behavior.md) — explicit, predictable behavior.
- `crates/wright-engine/src/foundry/charge/git.rs` — snapshot fetch and
  clone implementation.

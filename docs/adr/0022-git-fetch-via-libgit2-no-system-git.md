# ADR-0022: Git Source Fetching via libgit2, Never the System `git`

## Status

Accepted

## Context

Wright fetches `git` plan sources into a cached bare repository before building.
Two implementation paths exist for every git operation:

1. **libgit2** (via the `git2` crate), statically linked into the `wright`
   binary.
2. **Shell out** to the system `git` executable through `std::process::Command`.

The system `git` binary has materially better support for some operations — in
particular **shallow fetch negotiation**. libgit2's shallow support (libgit2
1.8.x) is incomplete: re-fetching into an existing shallow repository can abort
with an `ErrorClass::Odb` "object not found" error when upstream advertises a
thin pack whose base objects lie beyond the local shallow boundary. This is
tempting to "fix" by calling `git fetch --depth N` instead.

But Wright is a **self-contained package manager**. Its expected runtime
environments include:

- Bootstrap / stage-0 systems that do not yet have `git` installed.
- Minimal containers and `FROM scratch`-style images.
- Recovery environments where the toolchain is deliberately bare.

A package manager that shells out to `git` to fetch sources creates a
**bootstrap paradox**: the tool used to install packages would itself depend on
a package being pre-installed. The same argument applies to any external
runtime binary on the fetch path.

## Decision

**All git source operations go through libgit2. Wright never shells out to a
system `git` binary on the fetch path.**

We accept libgit2's weaker shallow-fetch behavior as a fixed constraint and
design around it rather than reaching for the system `git`:

- **Shallow fetches request only the single ref being built**, stored in a
  private `refs/wright/<ref>` namespace, instead of mirroring `refs/heads/*`
  and `refs/tags/*`. This removes unrelated upstream branches — which active
  repos routinely rebase or force-push — from the shallow negotiation, which is
  the dominant trigger of spurious `Odb` errors.
- **Full (non-shallow) fetches still mirror** all heads and tags, so arbitrary
  pinned commit hashes remain resolvable. A full clone has no shallow boundary,
  so mirroring is safe there.
- **When an `Odb` error does occur, it is treated as a shallow-cache refresh**,
  not as upstream history rewriting. The cache is removed and re-cloned shallow
  (cheap, since depth is small), and the event is logged at `debug` — Wright
  does not accuse the upstream of a force-push for what is usually a local
  libgit2 limitation.

See `src/foundry/charge.rs` (`git_fetch_attempt`, `local_fetch_ref`) for the
implementation.

## Consequences

### Positive

- **Zero runtime dependency on `git`.** Wright can fetch git sources on a bare
  system, satisfying its self-bootstrapping requirement.
- **Reproducible, statically-linked behavior** — fetch semantics do not drift
  with whatever `git` version happens to be on the host.
- Shallow updates of active upstreams no longer misreport "upstream history
  rewritten" on every build (the original motivating bug).

### Negative / Tradeoffs

- We forgo the system `git`'s more robust shallow protocol. Some shallow
  re-fetches still fall back to a full shallow re-clone instead of a true
  incremental deepen. For small depths this cost is acceptable.
- Future contributors may be tempted to "just call `git`" when they hit a
  libgit2 limitation. **That is prohibited on the fetch path by this ADR.**
  A libgit2 limitation must be worked around within libgit2, or escalated by
  superseding this ADR — not by adding a `Command::new("git")`.
- Wright remains exposed to libgit2 bugs and its release cadence for any future
  protocol improvements.

## References

- [ADR-0004](0004-no-magic-behavior.md) — explicit, predictable behavior; this
  ADR extends that principle to "no hidden external-binary dependency".
- `src/foundry/charge.rs` — git fetch implementation.
- libgit2 shallow-clone support tracking: libgit2/libgit2#3058.

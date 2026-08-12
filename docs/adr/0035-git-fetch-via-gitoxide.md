# ADR-0035: Git Source Fetching via gitoxide (gix)

## Status

Accepted

## Context

[ADR-0022](0022-git-fetch-via-libgit2-no-system-git.md) mandated libgit2 (via
the `git2` crate) for all git source operations, and prohibited shelling out
to the system `git`. That decision satisfied Wright's self-bootstrapping
requirement, but it carried two structural costs:

1. **Build time.** `git2` was built with `vendored-libgit2` +
   `vendored-openssl`, so every clean build compiled OpenSSL from source.
   Measured with `cargo build --timings`, `openssl-sys` alone accounted for
   roughly 32s of a 46s clean build — the single largest item by far.
2. **Two TLS stacks.** HTTP downloads (`reqwest`) already used `rustls`,
   while git fetches went through OpenSSL. The binary shipped two
   independent TLS implementations, and only one of them (OpenSSL) required
   a C toolchain and `perl`/`make` at build time.

Since ADR-0022 was written, [gitoxide](https://github.com/GitoxideLabs/gitoxide)
(`gix`) has matured: it is a pure-Rust git implementation whose blocking
network client can ride on `reqwest` + `rustls`, supports shallow fetch with
explicit depth, and keeps the no-system-`git` guarantee that ADR-0022 was
written to protect.

## Decision

**All git source operations go through `gix` (gitoxide), statically linked,
with HTTPS handled by `reqwest` + `rustls`. Wright never shells out to a
system `git` binary on the fetch path — that invariant from ADR-0022 is
unchanged.**

Concretely, in `crates/wright-engine/src/foundry/charge/`:

- Fetches use anonymous remotes (`Repository::remote_at`) with refspecs
  passed explicitly to `prepare_fetch`. The persisted `origin` remote that
  ADR-0022 needed to work around libgit2's shallow-negotiation defect
  (libgit2/libgit2#1430) is gone; gix does not consult persisted remote
  configuration during negotiation.
- Shallow fetches set `Shallow::DepthAtRemote(depth)` on the fetch
  preparation and store the single requested ref in the private
  `refs/wright/<ref>` namespace, exactly as before. Full fetches mirror
  `refs/heads/*` and `refs/tags/*`, exactly as before.
- Cache-refresh retry semantics are re-based on gix error classification:
  a failed **incremental** fetch is treated as a stale shallow cache only
  when the receive-phase error is non-spurious (per
  `IsSpuriousError::is_spurious`) and is neither a missing-remote-ref
  (`NoMapping`) nor a transport-level failure. Connection/handshake errors
  and transient network errors are terminal immediately, as before.
- The extract stage no longer clones the cache repository; it initializes
  an empty repository, fetches the needed refs from the local cache path
  with explicit refspecs, and checks out the resolved tree via the index.

This ADR supersedes [ADR-0022](0022-git-fetch-via-libgit2-no-system-git.md).

## Alternatives

### Keep libgit2

The vendored OpenSSL build dominates clean-build time and duplicates the
TLS stack. Dropping `vendored-openssl` would trade build time for a runtime
dependency on system OpenSSL, weakening the self-contained binary story
that motivates this project. Rejected.

### Shell out to the system `git`

Still prohibited — the bootstrap paradox from ADR-0022 stands. Rejected.

### gix with the curl HTTP transport

`blocking-http-transport-curl` would introduce libcurl (and, depending on
feature selection, another TLS backend). The reqwest transport reuses the
TLS stack Wright already ships. Rejected.

## Consequences

### Positive

- **OpenSSL, libgit2, and libssh2 leave the dependency tree entirely.** The
  clean build no longer compiles any C TLS code; `perl` is no longer needed
  at build time.
- **One TLS stack.** Git HTTPS and plain HTTPS both go through `rustls`;
  certificate handling and root stores are configured in exactly one place.
- **Fully self-contained fetch path.** `gix` is pure Rust; the wright binary
  gains no runtime dependency on system git, OpenSSL, or libssh2.
- The libgit2-specific workarounds documented in ADR-0022 (persisted
  `origin` remote, ODB-error-driven cache refresh) are replaced by
  simpler, explicit mechanisms.

### Negative / Tradeoffs

- **`ssh://` git sources now require a system `ssh` client.** libgit2
  shipped libssh2 statically; gix spawns the platform `ssh` binary, as git
  itself does. `https://`, `git://`, and local-path sources remain fully
  self-contained. Wright's documentation never promised ssh-based sources;
  plans that need them must ensure `ssh` exists on the build host.
- **gix is pre-1.0.** Its API evolves between minor releases, so upgrades
  require code adjustments. The dependency is pinned (`gix = "0.76"`) and
  upgrades are their own PRs.
- **The pin is deliberately not the newest gix.** From `gix-transport`
  0.52.1 onward (used by gix ≥ 0.77), the reqwest transport moved to
  reqwest 0.13, whose rustls backend is hard-wired to `aws-lc-rs` (a C
  build requiring `cmake`) and `rustls-platform-verifier` (which reads the
  *system* certificate store on Linux). Both contradict Wright's
  bootstrap-first stance: gix 0.76 pairs with reqwest 0.12, keeping the
  build `cmake`-free and the trust store embedded (`webpki-roots`), and
  unifying with the reqwest version Wright already ships. Revisit when a
  newer gix offers a ring/embedded-roots path again.
- Shallow-fetch failure modes differ from libgit2's: negotiation is
  computed client-side, so the `ErrorClass::Odb` failure pattern from
  ADR-0022 does not occur; the stale-cache refresh now keys off the
  classification described above. If an incremental shallow fetch keeps
  failing, the remedy is unchanged: delete the cache and re-clone.

## References

- [ADR-0022](0022-git-fetch-via-libgit2-no-system-git.md) — the decision
  this one replaces; its no-system-`git` invariant is retained.
- [ADR-0004](0004-no-magic-behavior.md) — explicit, predictable behavior.
- `crates/wright-engine/src/foundry/charge/git.rs` — fetch implementation.
- `crates/wright-engine/src/foundry/charge/extract.rs` — checkout from the
  local cache.

# ADR-0023: Parts Are Maintenance-Ledger Artifacts, Not Distribution Products

## Status

Accepted

## Context

Wright builds everything from source for the single machine it maintains. The
`.wright.tar.zst` binary part is produced as step 3 of every delivery
(resolve → build → seal → deploy) and kept in the local inventory
(`parts_dir` plus the archive database). It is used for reuse (skip rebuilds),
rollback (reinstall a prior state), and inspection.

Mature binary distributions (apt, pacman, dnf) treat the binary package as the
product: repositories, signatures, mirrors, and delta transports exist to move
that product between machines safely. Wright has none of this — no repository
index, no signing, no binary cache protocol — and an empty `src/repo/`
directory sat in the tree implying one was coming.

That ambiguity made the design illegible. Read as an unfinished distribution
system, Wright is missing its most important pieces. Read as a maintenance
ledger for one machine, it is complete. The two readings demand opposite
roadmaps (trust infrastructure vs. audit fidelity), so the position must be
recorded explicitly.

A second ambiguity follows from the first. Installing a foreign part already
works mechanically (`wright merge --path /path/to/part.wright.tar.zst`), and
people will do it. Today `.PARTINFO` carries only install-time facts (name,
build date, runtime deps, relations, backup files, plan identity). A part in
the inventory cannot answer "which plan content produced you, from which
sources?" — a blind spot for the ledger itself, and doubly so for a part that
arrived from another machine.

## Decision

### 1. No repository subsystem

Wright will not grow remote repository indexes, publish/fetch protocols,
mirrors, or a binary cache. The unit of exchange between machines is **plan
source**: another machine converges by building the same plans, not by
importing built parts. The empty `src/repo/` directory is removed so the tree
no longer advertises a forthcoming subsystem.

### 2. Parts are ledger entries

The authoritative purposes of a `.wright.tar.zst` part are, in order:

1. **Deploy** — carry the build that just happened onto the live root.
2. **Reuse** — let the inventory satisfy a future install without rebuilding.
3. **Rollback** — reinstall a previously deployed state.
4. **Audit** — record what was deployed and where it came from.

Distribution is a non-goal. Features are evaluated against the ledger
purposes; "would help share parts between machines" is not, by itself, a
justification.

### 3. Sharing is tolerated, not supported

Installing a part built elsewhere remains possible — refusing an explicit
operator action would violate the no-magic principle (ADR-0004). But it is
the operator's trust decision, equivalent to running a downloaded installer.
Wright will not build trust machinery (signing, key distribution, transport
verification) to bless the practice. Documentation states plainly: to put
software on a second machine, share the plan, not the part.

### 4. Provenance becomes part of the part format

Because the part is a ledger entry, `.PARTINFO` must be able to answer "what
produced this part". A `[provenance]` section is added with descriptive —
not cryptographically attested — facts recorded at seal time:

| Field | Content |
|-------|---------|
| `plan_checksum` | SHA-256 of the plan source that produced the part |
| `source_checksums` | per-source checksums as verified at charge time |
| `wright_version` | version of the `wright` binary that sealed the part |
| `isolation` | isolation mode the build ran under |

This serves the local ledger first: `wright check` / `wright doctor` can flag
an installed part whose recorded plan checksum no longer matches local plan
source (drift detection), and `wright history` can tie a deployment to exact
inputs. Incidentally it makes a foreign part *auditable* — the recipient can
see what allegedly produced it — without pretending to make it *trusted*.
This is the same "record facts, do not enforce" stance as ADR-0016. Readers
must treat the section as optional: parts sealed before this ADR do not carry
it.

Field plumbing (seal-time collection, `.PARTINFO` serialization, database
columns, `check`/`doctor` surfacing) is implementation follow-up to this ADR.

## Alternatives considered

**Build a repository and signing stack (apt/pacman model).** Rejected.
Distribution is not the goal, and key management, index formats, and mirror
tooling would dominate the maintenance budget of a tool meant to keep one
machine sailing.

**Nix-style binary substituters.** Rejected. Substituters only pay off with
content-addressed, reproducible identity, and ADR-0016 already rejected
hash-based identity as foreign to Wright's plan-centric, human-readable
model. Plan-source sharing reaches the same end state (the other machine
converges) through the front door.

**Forbid installing foreign parts.** Rejected. `merge --path` on a file the
operator chose is an explicit action; blocking it is magic in the prohibitive
direction. The honest posture is tolerate + document + make auditable.

**Sign parts without a repository.** Rejected. A signature without a key
distribution and revocation story is security theater. Provenance fields give
the audit value; they do not impersonate a trust system.

## Consequences

### Positive

- The design becomes legible: Wright is a *complete* maintenance ledger, not
  an *incomplete* distribution system. Roadmap questions ("where is the repo
  support?") have a recorded answer.
- Provenance closes the ledger's audit blind spot: every part can be tied to
  the exact plan content and sources that produced it, and drift between
  installed parts and current plan source becomes detectable.
- Scope stays small: no key management, no index format, no transport
  security surface.

### Negative

- Multi-machine users rebuild on every machine. This is the cost of the
  model, partially offset by inventory reuse per machine. If it ever becomes
  unacceptable, the answer is a future ADR superseding this one — not quiet
  scope creep.
- Foreign-part installs remain trust-on-first-use with no cryptographic
  recourse. Mitigated only by documentation and auditability.
- `.PARTINFO` grows; old parts lack the `[provenance]` section, so all
  readers must treat it as optional indefinitely.

## Related

- [ADR-0004: No implicit magic behavior](0004-no-magic-behavior.md) — why
  foreign-part installs are not blocked.
- [ADR-0016: Advisory runtime dependencies](0016-advisory-runtime-dependencies.md)
  — the "registry records facts, not enforcement" stance that provenance
  extends to part origins.
- [ADR-0017: Plan source as single dep truth](0017-plan-source-single-dep-truth.md)
  — plan source as the authoritative input; provenance records *which* plan
  source.
- `docs/explanation/distribution-model.md` — user-facing exposition of this
  decision.

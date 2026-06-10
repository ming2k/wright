# Distribution Model

Wright has no package repositories, no signing keys, and no binary cache —
and not because those pieces are missing. This page explains the position;
[ADR-0023](../adr/0023-parts-as-maintenance-ledger.md) records the decision.

## The part is a ledger entry, not a product

In a binary distribution (apt, pacman, dnf), the built package is the
product: the entire system — repositories, signatures, mirrors — exists to
move that product onto other machines safely.

Wright inverts this. The product is the **maintained machine**. The
`.wright.tar.zst` part is a record that a maintenance action happened: it
carries the build that was just deployed, lets the inventory satisfy a future
install without rebuilding, allows rolling back to a previously deployed
state, and documents what was put on the system and where it came from. In
the ship metaphor, parts are the workshop's stockroom and logbook — not a
catalogue for other ships.

This is why the inventory is local-only by design. A repository would scale
*trust and compute* across machines; Wright scales *maintenance history*
across time on one machine.

## How software reaches a second machine

Share the **plan**, not the part. A plan is small, readable, and reviewable;
the receiving machine builds it in its own sandbox against its own sysroot,
and the result lands in its own ledger with full provenance. This is the same
convergence path as any other install, so nothing about the second machine
becomes a special case.

## Sharing built parts: tolerated, not supported

`wright merge --path /path/to/foo.wright.tar.zst` will install a part built
on another machine. Wright does not block this — refusing an explicit
operator action would be magic in the prohibitive direction
([ADR-0004](../adr/0004-no-magic-behavior.md)). But understand what it is:
you are trusting the sender and the transport completely, exactly as if you
ran a downloaded installer. There is no signature to check because Wright
deliberately has no key infrastructure to make such a check meaningful.

What Wright does give you is **auditability**. A part's `.PARTINFO` records
provenance — the checksum of the plan source that produced it, the source
checksums verified at fetch time, and the sealing `wright` version — so you
can see what a foreign part claims to be made from. Provenance is descriptive,
not cryptographically attested; it supports inspection, not trust.

## Why provenance exists even without distribution

Provenance is not a concession to part-sharing — it serves the local ledger
first. With it, `wright check` and `wright doctor` can detect drift between
an installed part and the current plan source, and `wright history` can tie a
deployment to its exact inputs. A ledger whose entries cannot state their
origin has a blind spot; this closes it.

## What would change this

If multi-machine convergence ever becomes a first-class goal, the honest path
is a new ADR superseding ADR-0023 — likely bringing reproducible-build
guarantees and a real trust story with it — not a repository bolted onto the
side.

# Wright Engineering Standards

This directory is the authoritative engineering and documentation standards suite for Wright.

The suite is deliberately flat and lives inside the contributor space. Wright does not
vendor an external documentation meta-framework, maintain a separate governance silo, or
synchronize a protocol version: every document here addresses a current engineering concern
in this repository and names the mechanism that enforces it.

---

## Standards Directory

| Document | Purpose & Authority |
| :--- | :--- |
| [`architecture.md`](architecture.md) | Workspace topology, crate ownership, dependency direction, and architectural red-lines. |
| [`invariants.md`](invariants.md) | Invariant admissibility, the constraint matrix, and the registry of `[INV-*]` families. |
| [`adr.md`](adr.md) | ADR qualification (the 3-Question Test), lifecycle, frontmatter schema, and reference integrity. |
| [`rfc.md`](rfc.md) | Deliberation protocol for cross-cutting proposals that precede an ADR. |
| [`taxonomy.md`](taxonomy.md) | Diátaxis layout, audience firewall, root whitelist, and content routing. |
| [`style.md`](style.md) | Markdown conventions, link contracts, heading hierarchy, and voice. |
| [`workflow.md`](workflow.md) | Pull-request gates, code-to-doc synchronization, and the release checklist. |
| [`postmortems.md`](postmortems.md) | Blameless incident analysis and defense-in-depth action items. |

For hands-on work, start with:

- [`../contributing.md`](../contributing.md) — build prerequisites, test commands, commit style, and the PR workflow.
- [`../module-layout.md`](../module-layout.md) — the living crate-by-crate layout and placement rules.
- [`../tracing-output-design.md`](../tracing-output-design.md) — the Cargo-style output model every command follows.
- [`../isolation-pitfalls.md`](../isolation-pitfalls.md) — OverlayFS and `ETXTBSY` traps in sandboxed builds.

---

## Core Principles

1. **Mechanisms over exhortations.** A rule must be enforced by a compiler check, a crate
   boundary, or a deterministic test. A rule with no enforcement mechanism is an aspiration,
   and this suite does not record aspirations as invariants.
2. **Anti-inflation.** Separate the four categories in [`invariants.md`](invariants.md):
   system invariants, domain contracts, design specifications, and implementation notes. Do
   not mint an `INV-` tag for a layout constant.
3. **Flat and pragmatic.** No nested `core/` directories, no `profiles/` layer, and no
   parallel copy of an upstream standard. A document earns its place by describing how this
   repository actually works.
4. **Self-contained lineage.** Every decision citation resolves to a canonical record in
   [`docs/adr/`](../../adr/index.md). Do not cite an external ADR number that this repository
   does not contain.

---

## Mechanical Gates

The documentation rules in this directory are checked, not merely stated:

```bash
cargo test --test doc_governance   # links, audience firewall, root whitelist, ADR frontmatter
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

[`taxonomy.md`](taxonomy.md) §2 and [`style.md`](style.md) §1 define what the documentation
test rejects. [`workflow.md`](workflow.md) §1 defines the full pull-request gate.

# Repository Governance

Top-level governance charter, decision hierarchy, review gates, and documentation standards for `wright`.

| Section | Purpose |
|---------|---------|
| [Documentation Governance](documentation/core/index.md) | Clean-Break Architecture (Protocol v5.0.0): 4D spatial coordinate tensor, codified invariants constitution, operational lifecycle, and presentation syntax |

## Core Meta-Governance
- [Taxonomy](documentation/core/taxonomy.md): 4D spatial coordinate tensor (Temperature x Lifecycle x Audience x Mode).
- [Invariants](documentation/core/invariants.md): Codified constitution of numbered system invariants (`INV-*`).
- [Workflow](documentation/core/workflow.md): Code-to-doc trigger matrix, PR review gates, standard intake SOP, and adoption.
- [Style Guide](documentation/core/style.md): Technical voice, structural syntax, link contracts.
- [Repository Contracts](documentation/contracts.md): Active profiles and directory layout bindings.

## Active Domain Profiles
- **Architecture**: [Architecture Profile](documentation/profiles/architecture/index.md) (`adr.md`, `living-snapshot.md`, `rfc.md`).
- **Validation**: [Validation Profile](documentation/profiles/validation/index.md) (`acceptance.md`, `testing.md`).

## Governance Structure & Decision Hierarchy

1. **Architecture Decision Records (`docs/adr/`)** — Durable, immutable architectural records for significant structural choices.
2. **Repository Governance (`docs/governance/`)** — Top-level standards, intake filters, and documentation governance.
3. **Internal Contributor Guides (`docs/dev/`)** — Firewall-protected guides for local builds, testing isolation, and release procedures.
4. **User-Facing Diátaxis (`docs/tutorials/`, `docs/how-to/`, `docs/reference/`, `docs/explanation/`)** — External documentation organized by user learning mode.

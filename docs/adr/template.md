---
id: ADR-NNNN
title: "[Title of Decision]"
status: draft # [draft | accepted | superseded | rejected | deprecated]
date: YYYY-MM-DD
scope: [e.g., core/engine, storage/ledger, cli]
superseded_by: null # e.g., ADR-0042
negative_knowledge: true
---

# NNNN. [Title of Decision]

- Status: Draft | Accepted | Rejected | Deprecated | Superseded by [ADR-NNNN](NNNN-slug.md)
- Date: YYYY-MM-DD
- Deciders: [Names / GitHub handles]
- Consulted: [Names / GitHub handles]
- Informed: [Names / GitHub handles]

---

## Context and Problem Statement

Describe the forces, constraints, and problem that require a durable architectural decision.

## Decision Drivers

- [Driver 1, e.g., deterministic execution]
- [Driver 2, e.g., cross-crate dependency boundaries]

## Considered Options

- [Option 1: e.g., Proposed approach]
- [Option 2: e.g., Alternative approach]

## Decision Outcome

Chosen option: "[Option 1]", because [justification, e.g., fulfills latency budget and guarantees fail-closed isolation].

### Positive Consequences

- [Positive consequence 1]
- [Positive consequence 2]

### Negative Consequences

- [Trade-off or operational cost 1]
- [Trade-off or operational cost 2]

## Rejected Alternatives & Negative Knowledge

### Why [Option 2] Was Discarded

Detail why alternative solutions were evaluated and rejected, documenting explicit failure modes to prevent regressive re-exploration.

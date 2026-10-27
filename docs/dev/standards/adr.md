# Architectural Decision Records (ADR) Standards

This document defines when an ADR is warranted, how it changes state, what its frontmatter must
contain, and how citations stay resolvable.

---

## 1. Qualification: The 3-Question Test

Author an ADR only if a proposal answers **at least two** of these questions with yes:

1. **High reversal cost** — undoing the decision would require a migration, a data-layout
   change, or a multi-crate refactor.
2. **Cross-boundary blast radius** — it changes crate boundaries, a public CLI surface, a
   persisted format, isolation policy, or a performance/safety guarantee.
3. **Binding red-lines** — it creates a constraint that future code and reviews must follow.

### Rejected patterns

- **Triviality smear**: an ADR for choosing a serializer, naming an internal helper, or
  tuning a log line.
- **Ephemeral workaround**: a temporary bug fix or environment hack.
- **Toothless declaration**: a preference with no enforceable consequence.
- **Post-hoc justification**: an ADR written after the code landed, with no rejected
  alternatives and no real deliberation.

When in doubt, write an entry in `CHANGELOG.md` or an explanation document instead.

---

## 2. Lifecycle

ADRs transition in place through frontmatter. Files are never moved to an `archive/`
directory and are never renumbered.

```text
       [ proposed ] ───► [ accepted ] ───► [ superseded ]
             │
             └────────► [ rejected ]
```

| Status | Meaning |
| :--- | :--- |
| `proposed` | A decision under deliberation. It does not bind code yet. |
| `accepted` | Ratified; the decision is the current contract. |
| `superseded` | Replaced by a later ADR; `superseded_by` is mandatory. |
| `rejected` | Considered and discarded; retained as negative knowledge. |
| `deprecated` | No longer applicable because the subsystem was removed. |

Accepted ADRs are immutable in substance. A changed decision gets a new ADR; the old one's
status becomes `superseded` with a pointer. Clarifying a typo, adding a link, or normalizing
frontmatter is not a substantive edit.

---

## 3. Frontmatter Schema

Every file matching `docs/adr/[0-9]*.md` starts with YAML frontmatter:

```yaml
---
id: ADR-NNNN
title: "Title of the Decision"
status: accepted # proposed | accepted | superseded | rejected | deprecated
date: YYYY-MM-DD
scope: [core/foundry, cli]
superseded_by: null # ADR-NNNN when status is superseded
negative_knowledge: true
---
```

- `id`, `title`, `status`, and `date` are required.
- `status` must be one of the five values above.
- `superseded_by` is required when `status: superseded` and must point to an existing ADR.
- `scope` and `negative_knowledge` are recommended, not required; older records predate them.

---

## 4. Document Structure

Start from [the ADR template](../../adr/template.md). A complete record contains:

1. **Context and problem statement** — the forces and the failure being addressed.
2. **Decision drivers** — the constraints that discriminate between options.
3. **Considered options** — including the one chosen.
4. **Decision outcome** — what is now true, and its consequences.
5. **Rejected alternatives & negative knowledge** — why each other option failed. This section
   is mandatory; an ADR without it is a post-hoc justification.
6. **Invariants** — if the decision creates a new `[INV-*]` family, define each tag and name
   its enforcement mechanism (see [`invariants.md`](invariants.md)).

Accepted records carry a status list immediately after the H1:

```markdown
# NNNN. Title

- Status: Accepted
- Date: YYYY-MM-DD
- Deciders: Core Engineering Team
```

---

## 5. Reference Integrity

1. **Every `ADR-NNNN` citation resolves.** The target file is `docs/adr/NNNN-*.md`, and the
   record appears in [`docs/adr/index.md`](../../adr/index.md).
2. **No phantom numbers.** An ADR number that does not exist must not appear in prose, code
   comments, or commit messages.
3. **Cite for inspection, repeat the rationale.** A citation is a pointer; the sentence around
   it must still explain the decision to a reader who follows the link.
4. **Index stays current.** Adding an ADR means adding its row to the index in the same change.

The documentation test (`cargo test --test doc_governance`) enforces the frontmatter schema,
the status enum, `superseded_by` on superseded records, and index coverage.

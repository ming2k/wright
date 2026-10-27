# Request for Comments (RFC) Standards

An RFC is an exploratory proposal. An [ADR](adr.md) records a decision that has already been
made; an RFC is how a decision gets made when the answer is not obvious.

---

## 1. When to Write an RFC

Write an RFC when at least one of these is true:

- The problem has competing solutions whose trade-offs are not settled.
- The change spans multiple crates or a persisted contract (registry schema, plan format,
  archive layout, CLI surface).
- The cost of reversal is high enough that implementation should not start before agreement.

Small, local, or already-decided changes do not need an RFC. Write the ADR directly.

---

## 2. Lifecycle

```text
  [ proposed ] ──► [ under review ] ──► [ approved ] ──► [ migrated to ADR ]
                          │
                          └──► [ closed ]
```

| State | Meaning |
| :--- | :--- |
| `proposed` | Draft circulated for feedback. |
| `under review` | Active review with a stated decision deadline. |
| `approved` | Consensus reached; implementation may begin. |
| `migrated to ADR` | The landed decision is recorded as an ADR. The RFC becomes historical. |
| `closed` | Withdrawn or rejected before implementation. |

RFCs are working documents. Once the decision lands, the ADR is the durable record and the RFC
is not edited further. Do not create a parallel RFC directory tree; an RFC lives where the
proposal is being developed (an issue, a draft document, or a commit on a branch) until it is
ready to migrate.

---

## 3. Required Content

An RFC must state:

1. **Motivation and problem statement** — what breaks or what cannot be done today.
2. **Proposed design** — interfaces, data models, state transitions, and concurrency
   semantics.
3. **Failure modes** — behavior under interrupted processes, malformed input, partial
   deployment, and rollback.
4. **Migration and compatibility** — how existing plans, archives, databases, and CLI users
   move to the new behavior.
5. **Alternatives considered** — including the option of doing nothing.
6. **Unresolved questions** — the known unknowns reviewers must address.

---

## 4. From RFC to ADR

When the proposal is approved and implemented:

1. Write the ADR using [`adr.md`](adr.md) and the [template](../../adr/template.md).
2. Carry the rejected alternatives into the ADR's **Rejected Alternatives & Negative
   Knowledge** section. That section is the part an RFC usually cannot supply yet.
3. Add the ADR to [`docs/adr/index.md`](../../adr/index.md).
4. Mark the RFC `migrated to ADR` and link the record.

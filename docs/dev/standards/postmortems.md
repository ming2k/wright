# Incident Postmortem Standards

A postmortem exists to find the systemic gap that allowed an unsafe state, not to find a person.

---

## 1. Blameless Rule

Do not attribute an incident to "human error", inattention, or a named individual. A human
action is a trigger, not a root cause. Ask instead:

- Why did the type system, test suite, or review gate permit this state?
- Why was the failure not detected before it reached a deployed system?
- What made the wrong action the easiest one to take?

---

## 2. Structure

Postmortems live in `docs/dev/postmortems/YYYY-MM-DD-<slug>.md` and contain:

1. **Summary** — impact, affected systems or plans, duration, and whether data or deployed
   files were corrupted.
2. **Timeline (UTC)** — when the defect was introduced, when it first manifested, when it was
   detected, when it was mitigated, and when the permanent fix was verified.
3. **Detection gap** — why the existing tests, CI gates, or logs did not catch it.
4. **Root cause** — the state transition, boundary leak, or missing check that made it possible.
5. **Action items** — each with an owner and a verifiable completion criterion.

---

## 3. Action Items

Every action item is one of:

- **Preventive** — a type, a crate boundary, or a validation that makes the bad state
  unrepresentable.
- **Detective** — a regression test, assertion, or log line that fails loudly next time.
- **Containment** — a limit that bounds the damage if the failure recurs.

Prefer preventive actions. "Add a note to the docs" is not an action item unless the missing
instruction is itself the systemic cause.

---

## 4. Relationship to Invariants and ADRs

If an incident reveals a durable architectural gap, the fix is an [ADR](adr.md) and, when the
new rule has a test, a new invariant family in [`invariants.md`](invariants.md). The postmortem
records what happened; the ADR records what is now required. Do not use a postmortem as the only
home for a rule future code must follow.

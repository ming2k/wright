# Invariant & Constraint Governance

This document defines what may be called an invariant in Wright, how invariants are cited, and
which mechanisms make them more than prose.

---

## 1. Core Principle: Mechanisms Over Exhortations

An invariant is a condition that must hold in every valid state of the system. A rule with no
compiler proof, dependency firewall, or deterministic test is not an invariant — it is an
aspiration, and it does not receive an `[INV-*]` identifier.

---

## 2. Constraint Admissibility Matrix

Every rule maps into exactly one category:

| Category | Scope | Identifier policy | Enforcement mechanism |
| :--- | :--- | :--- | :--- |
| **System Invariant** | Structural red-lines with cross-boundary blast radius: source identity, deployment permissions, privilege separation, isolation. | `[INV-<FAMILY>-NN]`, defined in an ADR | Regression test (`test_inv_*` or a named integration test), compiler type, or crate boundary |
| **Domain Contract** | State transitions inside one module or subsystem. | No global tag; expressed as a named type | Typed `enum`, private field, RAII guard, unit test |
| **Design Specification** | Output layout, log format, colour, truncation, defaults. | No `INV-` tag; design constant | Snapshot or layout test |
| **Implementation Note** | Local edge-case guard or workaround. | No `INV-` tag; ordinary comment | The regression test that motivated it |

If a proposed rule does not fit the first row, do not promote it to keep it visible. Put it in
the category that describes it.

---

## 3. The 3-Condition Gate for System Invariants

A rule may become a system invariant only if all three hold:

1. **High blast radius** — violating it corrupts state, deploys stale or wrong bytes, loses
   data, deadlocks a process, or inverts an architectural boundary.
2. **Durable architecture** — it is a foundational property, not a detail that routine
   feature work will redesign.
3. **Mechanically enforced** — the ADR names the test, type, or dependency rule that fails
   when the invariant is violated.

---

## 4. Invariant Registry

Every family currently in force:

| Family | Subject | Defining ADR |
| :--- | :--- | :--- |
| `[INV-SRC-01..06]` | Content-addressed source identity: one digest, local sources keyed by bytes, rooted stage chain, volatility abstains, reuse requires proof, unreadable sources fail closed. | [ADR-0054](../../adr/0054-content-addressed-source-identity.md) |
| `[INV-DRIFT-01..05]` | Plan drift covers the whole plan directory: provenance records content, outdatedness includes bundled files, unknown evidence abstains, plan-relative resolution, one derivation of source content. | [ADR-0055](../../adr/0055-plan-drift-covers-bundled-sources.md) |
| `[INV-COLD-01..08]` | Explicit cold builds: no implicit trigger; only pure caches; invalidate derived state before inputs; verified inputs are not caches; purges are journaled transactions; reclaimed bytes are inode-aware; one lock order; no overlapping storage layouts. | [ADR-0057](../../adr/0057-transactional-cold-purge-and-trust-boundary.md) |
| `[INV-PERM-01..07]` | Diagnostic accessibility, payload permission canonicalization, and umask immunity across build, seal, and deploy. | [ADR-0050](../../adr/0050-diagnostic-artifact-accessibility-and-permission-boundaries.md), [ADR-0053](../../adr/0053-payload-permission-canonicalization-and-triple-defense-umask-immunity.md) |
| `[INV-PRIV-01..06]` | Command privilege classes and the read-only query path. | [ADR-0044](../../adr/0044-command-privilege-classes-and-read-only-query-path.md) |
| `[INV-LAYER-01..03]` | Directory topology preservation across stage layers and the merged base. | [ADR-0051](../../adr/0051-directory-topology-preservation-in-stage-layers.md) |

The enforcement named by each ADR is executable:

- `[INV-SRC-*]`, `[INV-DRIFT-*]`: `tests/integration/build_test.rs`,
  `tests/integration/install_pipeline_test.rs`.
- `[INV-COLD-*]`: `tests/integration/no_cache_test.rs`.
- `[INV-PERM-*]`: `tests/integration/permission_boundary_test.rs`.
- `[INV-PRIV-*]`: `tests/integration/output_test.rs` and the typed query path in
  `wright-registry`.
- `[INV-LAYER-*]`: `tests/integration/layering_test.rs`.

---

## 5. Citation and Lineage Rules

1. **Define before citing.** An `[INV-*]` tag in code, a test, or prose must resolve to the
   ADR that defines it. A phantom tag is a documentation defect and is removed or defined,
   never left to look authoritative.
2. **One definition.** The ADR is the source of truth. Other documents may restate the rule
   in their own words; they must not introduce a second numbering of it.
3. **Cite the mechanism.** When a document invokes an invariant, name the enforcing test or
   type. `[INV-SRC-01]` alone is a pointer, not an explanation.
4. **No external lineage.** Cite only ADRs present in [`docs/adr/`](../../adr/index.md).

---

## 6. Retirement

1. **Degazetting.** A rule that turns out to be a design preference (layout, message wording,
   a default value) loses its `INV-` tag. Replace the tag with the design constant or the
   ordinary comment that expresses it; do not leave a tombstone tag behind.
2. **Superseding.** When an invariant is replaced, the new ADR states the replacement and the
   old ADR is marked `status: superseded` with `superseded_by: ADR-NNNN`. The failure mode
   that motivated the old rule stays in the new ADR's negative knowledge.
3. **Removal.** When the subsystem is removed, the invariant is removed with it. The ADR
   remains as historical record.

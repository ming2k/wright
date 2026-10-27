# Engineering Workflow & Verification Gates

This document defines what must be true before a change merges and before a release is tagged.

---

## 1. Pull-Request Gates

Every pull request into `main` clears four gates:

```text
┌─────────────────────────────────────────────────────────────┐
│ 1. Documentation gate                                       │
│    cargo test --test doc_governance                          │
│    Links resolve; firewall holds; root whitelist holds;      │
│    ADR frontmatter and index are valid                       │
├─────────────────────────────────────────────────────────────┤
│ 2. Format gate                                              │
│    cargo fmt --all -- --check                               │
├─────────────────────────────────────────────────────────────┤
│ 3. Lint gate                                                │
│    cargo clippy --all-targets -- -D warnings                │
├─────────────────────────────────────────────────────────────┤
│ 4. Build and test gate                                      │
│    cargo build --all-targets                                │
│    cargo test                                               │
└─────────────────────────────────────────────────────────────┘
```

CI runs these as the `fmt`, `clippy`, and `test` jobs in
[`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml). The documentation test is part
of the test target, so a link or frontmatter defect fails the same job as a code defect.

Changes that touch privilege classes, deployment permissions, isolation, or the source-identity
pipeline must also pass the regression named by the relevant invariant in
[`invariants.md`](invariants.md) §4.

---

## 2. Code-to-Doc Synchronization

A change that modifies a documented surface updates that surface in the same commit. This is a
review gate, not a follow-up ticket.

| Changed surface | Required documentation |
| :--- | :--- |
| CLI command, flag, default, or error behavior | [`docs/reference/cli-reference.md`](../../reference/cli-reference.md), and `CHANGELOG.md` when user-visible |
| `plan.toml` schema or pipeline semantics | [`docs/reference/plan-manifest.md`](../../reference/plan-manifest.md) |
| Folio manifest (`<name>.toml`) | [`docs/reference/folio-manifest.md`](../../reference/folio-manifest.md) |
| Configuration key, default, or search path | [`docs/reference/configuration.md`](../../reference/configuration.md) |
| Registry schema or a new migration | [`docs/reference/database-design.md`](../../reference/database-design.md) |
| Build, charge, forge, seal, or cache behavior | [`docs/reference/build-mechanics.md`](../../reference/build-mechanics.md) and the relevant explanation page |
| On-disk layout or maintenance paths | [`docs/reference/local-inventory.md`](../../reference/local-inventory.md) |
| Crate boundaries or execution topology | New/amended ADR and [`architecture.md`](architecture.md) |
| Contributor build or test workflow | [`../contributing.md`](../contributing.md) |
| Documentation or review rules | The relevant file in this directory |

A pure internal refactor with no changed behavior needs no page update. If it changes a durable
boundary, it needs an ADR instead.

---

## 3. Pre-Release Checklist

Before tagging a release:

1. **Version coherence.** `[workspace.package].version`, the newest `CHANGELOG.md` section, and
   `wright --version` from a release build all state the same version. The changelog section
   describes the code being tagged, not the previous release.
2. **Documentation gate.** `cargo test --test doc_governance`.
3. **Format and lint.** `cargo fmt --all -- --check` and
   `cargo clippy --all-targets -- -D warnings` are silent.
4. **Full test suite.** `cargo test` passes; isolation-dependent tests may skip themselves when
   `unshare` is unavailable, and the skip is not counted as a pass for the release sign-off.
5. **ADR lineage.** Every `ADR-NNNN` citation resolves, and every file under `docs/adr/` is in
   [`docs/adr/index.md`](../../adr/index.md).
6. **Clean git state.** `git status --porcelain` shows no unintended changes and, critically,
   no `??` entries under `crates/`, `src/`, `tests/`, or `docs/`. A release is cut from a
   commit: an untracked module compiles locally and is absent from the tag.
7. **Changelog integrity.** Released sections are not rewritten.
8. **Release build.** `cargo build --release` succeeds locally, since the binary is the
   artifact.

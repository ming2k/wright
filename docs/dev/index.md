# Developer Documentation

Documentation for contributors and maintainers.

## Engineering Standards

- [Engineering Standards](standards/README.md) — the standards suite: architecture, invariants, ADRs, RFCs, taxonomy, style, workflow, and postmortems
- [System Architecture Governance](standards/architecture.md) — crate topology, dependency direction, and architectural red-lines
- [Invariant Governance](standards/invariants.md) — the constraint matrix and the `[INV-*]` registry
- [ADR Standards](standards/adr.md) — when and how to write a decision record
- [Documentation Taxonomy](standards/taxonomy.md) — Diátaxis layout, audience firewall, and routing
- [Documentation Style](standards/style.md) — link and Markdown conventions
- [Engineering Workflow](standards/workflow.md) — pull-request gates and the release checklist
- [Postmortem Standards](standards/postmortems.md) — blameless incident analysis

## Guides

- [Contributing](contributing.md) — prerequisites, build, test, commit style, and PR workflow
- [CLI Output & Tracing Design](tracing-output-design.md) — Cargo-style verb scrolling, span-driven persistent rows, error reporting, file-log conventions
- [Module Layout](module-layout.md) — the living crate-by-crate topology and placement rules
- [Isolation Race Handling](isolation-pitfalls.md) — ETXTBSY, OverlayFS, and mount-ordering traps

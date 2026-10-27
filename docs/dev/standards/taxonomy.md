# Documentation Taxonomy & Information Architecture

This document defines where documentation lives, who it is written for, and which directions a
link may cross.

---

## 1. The Four-Quadrant System (Diátaxis)

Public documentation is organized by what the reader is doing, not by what the topic is:

```text
               PRACTICAL                   THEORETICAL
          ┌─────────────────────────┬─────────────────────────┐
LEARNING  │  Tutorials              │  Explanation            │
          │  `docs/tutorials/`      │  `docs/explanation/`    │
          ├─────────────────────────┼─────────────────────────┤
WORKING   │  How-to guides          │  Reference              │
          │  `docs/how-to/`         │  `docs/reference/`      │
          └─────────────────────────┴─────────────────────────┘
```

| Directory | Reader intent | Voice |
| :--- | :--- | :--- |
| `docs/tutorials/` | Learn the system from zero | Second person, guaranteed steps, expected result at each step |
| `docs/how-to/` | Accomplish a specific task | Imperative, assumes basic competence, does not re-teach concepts |
| `docs/reference/` | Look up a fact | Tables and lists, complete, minimal narrative |
| `docs/explanation/` | Understand why | Discursive, may state trade-offs, links to ADRs for decision history |

If content seems to belong in two quadrants, it is two documents.

---

## 2. Documentation Spaces and Audiences

```text
┌─────────────────────────────────────────────────────────────┐
│ Public surface (users, operators, integrators)              │
│ docs/tutorials/, docs/how-to/, docs/reference/,             │
│ docs/explanation/                                           │
└──────────────────────────────┬──────────────────────────────┘
                               │ public must never link into
                               │ docs/dev/**
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Engineering surface (contributors, maintainers, agents)     │
│ docs/dev/ (standards, build and test runbooks)              │
│ docs/adr/ (canonical decision records)                      │
└─────────────────────────────────────────────────────────────┘
```

1. **Audience firewall.** A document in `docs/{tutorials,how-to,reference,explanation}/` must
   not contain a relative link into `docs/dev/`. Contributor documents may link outward to the
   public surface. The portal `docs/index.md`, `README.md`, and `CONTRIBUTING.md` are entry
   points: they may link into either surface.
2. **ADRs are dual-audience.** A public reader may follow a decision record. A contributor
   runbook is not public. Links from public documentation into `docs/adr/` are therefore
   allowed; links into `docs/dev/` are not.
3. **`docs/dev/` is the only internal space.** There is no `docs/governance/` and no vendored
   standards tree. Engineering rules live in [`docs/dev/standards/`](README.md).
4. **The firewall is checked.** `cargo test --test doc_governance` fails when a public
   document links into `docs/dev/`.

---

## 3. Repository Root Whitelist

Only canonical entry points may be Markdown files at the repository root:

| File | Purpose |
| :--- | :--- |
| `README.md` | Project pitch and the shortest path to a working binary |
| `CHANGELOG.md` | Release history |
| `CONTRIBUTING.md` | Contributor entry point; links into `docs/dev/` |
| `AGENTS.md`, `CLAUDE.md`, `GEMINI.md` | Local agent entry points. They are gitignored by design: permitted at the root, but not part of the repository. |
| `LICENSE`, `LICENSE.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md` | Legal and policy files required at the root |

Everything else belongs under `docs/`. The documentation test rejects any other root-level
`*.md` file.

---

## 4. Routing

Apply these in order and stop at the first match:

1. Records a decision that was made → `docs/adr/NNNN-<slug>.md`
2. Needed to build, test, or contribute → `docs/dev/`
3. Teaches the system step by step → `docs/tutorials/`
4. Solves a specific named task → `docs/how-to/`
5. Looks up a flag, schema field, config key, or file path → `docs/reference/`
6. Explains why the system behaves as it does → `docs/explanation/`
7. One-paragraph pitch and quick start → `README.md`

A rule about how to write or review code → `docs/dev/standards/` only when it is a project-wide
standard. A local convention belongs in the module's own documentation or a comment.

---

## 5. Lifecycle

- **Living documents** in the four public quadrants and `docs/dev/` are updated in the same
  change as the code they describe.
- **ADRs** are durable. They change state in place (see [`adr.md`](adr.md)) and are never
  rewritten to match new behavior.
- **Postmortems** in `docs/dev/postmortems/` are historical records; they are not edited to
  reflect later fixes (see [`postmortems.md`](postmortems.md)).

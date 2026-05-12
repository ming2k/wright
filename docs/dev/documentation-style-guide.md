# Documentation Style Guide

## Documentation Structure (Diátaxis)

Organize all documentation by reader intent. Ask "what is the reader trying to do right
now?" — not "what is this content about?".

### Directory layout

docs/tutorials/   — Learning-oriented. Second person ("you will…"). Guarantee success.
                    State expected outcome at every step.
docs/how-to/      — Task-oriented. Imperative mood. Titles always start with "How to".
                    Assume the reader knows the basics; do not re-explain concepts.
docs/reference/   — Lookup-oriented. Tables and lists only. No prose narrative.
                    Completeness over readability.
docs/explanation/ — Understanding-oriented. Discursive, opinionated allowed.
                    Link to ADRs for specific decision history.
docs/adr/         — Architecture Decision Records. Numbered (0001, 0002, …), immutable,
                    append-only. Once Accepted, never edit; write a new ADR to supersede.
docs/dev/         — Contributor docs only (setup, testing, release). Never mix with user docs.

Root-level markdown is limited to: README.md, CHANGELOG.md, CONTRIBUTING.md, LICENSE,
SECURITY.md, CODE_OF_CONDUCT.md.

### Routing — where does new content go?

Apply in order; stop at the first match:
1. Records a past architectural decision → docs/adr/NNNN-<slug>.md
2. Needed to set up dev environment or contribute → docs/dev/
3. Reader follows step-by-step to learn the system → docs/tutorials/
4. Reader is trying to accomplish a specific named task → docs/how-to/
5. Reader scans for a config key, API field, or CLI flag → docs/reference/
6. Explains why something works the way it does → docs/explanation/
7. One-minute pitch + minimal run command → README.md

If content seems to belong in two places, it is two documents — split it.

### When code changes require doc updates (same commit)

- Public API, CLI flag, or config key changed → update docs/reference/
- New user-discoverable feature added → add a how-to guide in docs/how-to/
- Install, build, or run steps changed → update README.md quick start + docs/tutorials/01-getting-started.md
- Dev environment or test commands changed → update docs/dev/
- Architectural decision made → write a new ADR in docs/adr/
- User-visible behavior changed → add entry under "Unreleased" in CHANGELOG.md
- Pure internal refactor with no user-visible effect → no doc change needed

### Never do

- Do not create a monolithic Documentation.md or Guide.md — split by Diátaxis category
- Do not duplicate the README quick start inside docs/ — link to it
- Do not add design rationale to reference pages — move to docs/explanation/ or an ADR
- Do not put option tables inside tutorials — link to docs/reference/ instead
- Do not edit an Accepted ADR — write a new one and update the old one's status to
  "Superseded by ADR-NNNN"
- Do not modify or delete files under docs/adr/ — ADRs are immutable and append-only
- Do not modify or delete files under src/database/migrations/ — migration history is immutable
- Do not edit or rewrite CHANGELOG.md history — entries are append-only
- Do not mix user docs and contributor docs — docs/dev/ is the firewall
- Do not create an empty directory without an index.md placeholder

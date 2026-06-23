# Contributing

Contributions are welcome. This guide covers the local workflow the CI
pipeline enforces. Read it once before opening a pull request.

## Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust | 1.85 or newer | `edition = "2024"` requires it |
| C toolchain | any | `cc` / `gcc` / `clang`, for vendored C deps |
| pkg-config | any | Locates system libraries at build time |

System libraries are needed for the non-vendored link steps:

```bash
sudo apt-get install -y libsqlite3-dev liblzma-dev libbz2-dev libzstd-dev \
  pkg-config perl make bubblewrap
```

`git2`, `openssl`, and `libgit2` are vendored by Cargo features; do not
install system copies. `bubblewrap` is optional — integration tests that
need namespace isolation skip themselves when it is unavailable.

## Build

```bash
cargo build                          # debug
cargo build --release                # release, matches installed binary
```

The build script regenerates shell completions and man pages under
`target/completions/` and `target/man/` whenever `src/cli/` changes. Those
artifacts are not committed.

## Run tests

The crate is a single binary; `cargo test` covers the whole project:

```bash
cargo test                           # all unit + integration tests
cargo test --test integration        # only tests/integration/
cargo test build_test                # filter by test name substring
```

Tests that require namespace isolation call
`should_skip_isolation_test` and exit silently when the kernel or the
container denies `unshare`. No test contacts the network: the `nginx`
fixture only validates manifest structure and never downloads the tarball.

## Code style

Two gates must pass before pushing:

```bash
cargo fmt --all                      # apply
cargo fmt --all -- --check           # CI mode: fails instead of writing
cargo clippy --all-targets -- -D warnings
```

CI fails the build on any `clippy` warning or any `rustfmt` diff. Run
both locally before opening a pull request.

## Commit conventions

Use [Conventional Commits](https://www.conventionalcommits.org/) with a
module scope, matching existing history:

```
feat(foundry): narrow shallow git fetch to the requested ref
fix(build): stop full recompile on every rerun-if-changed miss
docs: record ADR-0024 — work-directory source names
chore(release): release version 5.3.10
```

Common scopes seen in history: `foundry`, `charge`, `part`, `launch`,
`build`, `owner`, `database`, `transaction`, `release`. Pick the module
the change lives in; omit the scope only for cross-cutting changes such
as `docs:`.

## Pull request workflow

1. Open the PR against `main`.
2. CI runs three jobs in parallel: `rustfmt`, `clippy`, `build & test`.
   All three must be green.
3. If the PR touches code with a corresponding documentation surface
   (see [Update Checklist](documentation/update-checklist.md)), state in
   the PR description whether the documentation was updated or why not.
4. Squash-merge on approval; the commit title becomes the changelog
   source for release notes.

## Documentation changes

Wright follows the Diátaxis framework with strict routing. Before
writing or moving any documentation:

1. Read [Documentation Governance](documentation/index.md).
2. Route the content with [Routing](documentation/routing.md).
3. Match voice and formatting with the [Writing Style](documentation/style-guide.md).

Hard rules to keep in mind:

- `docs/adr/` records are immutable. To change a decision, write a new
  ADR and mark the old one `Superseded by ADR-NNNN`. See
  [ADR Workflow](documentation/adr-workflow.md).
- `CHANGELOG.md` entries are append-only. Do not edit released history;
  add under `Unreleased`.
- `src/database/migrations/*.sql` are immutable. To change schema, add a
  new numbered migration.
- Do not link from user-facing docs (`docs/tutorials/`,
  `docs/how-to/`, `docs/reference/`, `docs/explanation/`) into
  `docs/dev/`. The reverse direction is fine.

## Architecture context

New contributors should read these once to understand the lay of the
land before opening non-trivial PRs:

- [Module Layout](module-layout.md) — where each subsystem lives
- [CLI Output & Tracing Design](tracing-output-design.md) — the
  Cargo-style output model every command must follow
- [Isolation Race Handling](isolation-pitfalls.md) — OverlayFS and
  ETXTBSY traps that bite sandboxed builds

Architectural decisions are recorded as ADRs under `docs/adr/`. Read the
recent ones before proposing changes to established subsystems; the
index at `docs/adr/index.md` lists every decision and its status.

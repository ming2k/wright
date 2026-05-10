# Dependency Philosophy

Wright treats runtime dependencies as **advisory facts**, not enforced
invariants. The installed registry records what each part declares it needs;
it does not promise that those needs are met. This page explains the model
and why it is the right shape for a plan-centric, build-from-source
orchestrator.

## Three states, not one

A part can be in any combination of these three states:

| State | Means | Determined by |
|-------|-------|---------------|
| **registered** | The part exists in `wright.db`; its files are laid down on the target root. | An install / upgrade transaction completed. |
| **satisfied** | Every entry in `dependencies.required_name` resolves to another registered part (or to a `replaces` alias). | A query against current registry state. |
| **runnable** | The part actually executes — the dynamic loader can find every library, every dlopen target is present, every data file exists. | An attempt to run, or a pre-flight check. |

`registered` does not imply `satisfied`. `satisfied` does not strictly imply
`runnable` — dlopen, configuration, and data-file dependencies can still
break a satisfied part. Each state answers a different question; collapsing
them into a single "installed" notion is what package managers do, and it is
what Wright deliberately does not do.

## Why advisory

Wright is not a binary distribution. Users describe *intent* — a sysroot, a
minimal embedded image, a development host, a staged migration — and Wright
constructs that intent from plan source. Strict completeness fights intent:

- A sysroot prep deliberately installs a subset; missing runtime deps are
  the goal, not an error.
- A cross-target staging area is half-broken by design until it is shipped.
- A mid-upgrade transient (remove `libfoo`, install `libfoo-ng`) is briefly
  unsatisfied; the transient must be allowed to exist.
- A plan source typo produces a permanently unresolvable name; this should
  surface as a diagnostic, not cascade-fail every operation that touches it.

A registry that *records facts* and a separate diagnostic surface that
*reports problems* lets all of these workflows coexist. A registry that
*enforces obligations* must reject every one of them.

## What the schema enforces vs. observes

The installed registry has three relationship tables. They are not
symmetric in semantics — that asymmetry is intentional.

| Table | Schema | Behavior |
|-------|--------|----------|
| `dependencies` | `(part_id FK, depends_on TEXT, version_constraint TEXT?)` | **Advisory.** `depends_on` is a soft `plan:output` pointer; the target may or may not be registered. The `part_id` FK only locks ownership of the row to an actually-registered part. |
| `replaces` | `(part_id FK, name TEXT)` | **Advisory + transitional.** Records that `part_id` supersedes the named part during upgrades; used to resolve old references after a rename. |
| `conflicts` | `(part_id FK, name TEXT)` | **Hard constraint.** Install fails (without `--force`) if a conflicting part is registered. Conflicts are mutually exclusive by construction; advisory semantics are wrong for them. |

`dependencies` has no FK on `depends_on` and never will. Forcing the target
to exist before the depender can install was the broken assumption that
"installed implies runnable" and it is incompatible with every workflow
listed above.

## Why provides, build_deps, and link_deps are not in the registry

All three concern things that are true *during construction*, not after a
part is laid down on disk.

- **`build_deps`** (toolchain) — irrelevant once the binary is on disk.
  Removing the compiler does not break programs the compiler produced.
  Build deps live in plan source so the build pipeline can mount them; they
  are not persisted.
- **`link_deps`** (declared link relationships) — superseded by empirical
  evidence: a built binary's `DT_NEEDED` entries describe exactly what the
  dynamic loader will request. Declared link deps are at best a hint and at
  worst out of sync with reality. The registry tracks runtime needs only;
  whether those needs were caused by linking, dlopen, or data-file lookup
  is irrelevant.
- **`provides`** (virtual aliasing) — a Debian-style abstraction. In a
  plan-centric world every output is a first-class identifier; alternatives
  (musl vs. glibc, postfix vs. sendmail) are build-time variant decisions,
  not runtime resolution decisions. Renames and migrations are handled by
  `replaces`, which is more honest because it names the specific old
  identity rather than fabricating a virtual one.

## How drift is handled

The two failure modes that look frightening on first read:

**Typos and forgotten declarations** in a `runtime_deps` list would
silently produce parts that fail at runtime with cryptic loader errors.
Defended by the package-time **ELF lint**: each output's staged binaries
are scanned for `DT_NEEDED` entries; the providing parts are looked up in
a SONAME index built from known archives; if the binary needs a part the
plan source does not declare, the package step fails with the exact list
of missing declarations. Plan source remains the single source of truth
— **the lint never injects derived data into PARTINFO or the database**.
See [ADR-0017](../adr/0017-plan-source-single-dep-truth.md).

**Renames** produce stale references when the dependency target's output
name changes. Defended by `replaces`: the new plan declares
`replaces = ["old-name"]` and resolution falls through to the new part.
`wright lint` enforces that rename diffs against the prior release include
the matching `replaces` entry.

## ELF lint policy at a glance

| Mismatch | Meaning | Action |
|----------|---------|--------|
| ELF needs `X`, plan doesn't declare it | Forgotten declaration; binary will fail to load | **Error** — package step fails |
| Plan declares `Y`, no ELF edge to it | Likely dlopen / data-file dep | **Warn** — author keeps or removes |
| ELF needs SONAME `Z`, no part provides it | Vendored, host-provided, or missing | **Warn** — author investigates |

The asymmetry is intentional: forgotten declarations are silent footguns,
surplus declarations are noise. Surplus may be legitimate (dlopen targets
are invisible to the linker), so only the author can decide.

## Operational shape

The diagnostic and recovery surface:

- `wright check` — scans the registry for unresolved `dependencies` and
  reports them. Read-only. Resolution walks `parts.name` first, falls
  through to `replaces` for renamed targets.
- `wright check --deep` — additionally walks each installed part's ELF
  binaries and verifies each `DT_NEEDED` SONAME against the installed
  files table. Catches forgotten-declaration footguns that registry-level
  resolution misses. Optional `[PART]` argument restricts the scope.
 - `wright check --deep <part>` — focused check on a single part before
   invoking it.

`wright remove` warns when removing a depended-on part but does not block:
the user decides whether to accept the broken state. The dependency edge in
the registry survives the removal as a recorded unsatisfied claim, ready to
be repaired by a later install.

## What this rules out

This model deliberately does not provide:

- **An invariant that "every installed part is runnable"** — there is none,
  by design. Tools surface the question; users answer it.
- **Database-level removal protection on dependents** — a soft TEXT pointer
  cannot drive `ON DELETE RESTRICT`, and the binary semantics it would give
  (refuse vs. cascade) are too coarse anyway. Application-level warnings
  with `--force` override are the right shape.
- **Virtual-name resolution** — `provides` does not exist. Depend on a
  specific `plan:output` or accept the constraint via `replaces`.
- **Runtime version enforcement** — `version_constraint` is preserved as
  diagnostic metadata so `wright check` can flag mismatches; nothing in the
  install path rejects a version-constraint failure. Variants are a
  build-time choice, not a runtime resolution choice.

## Related

- [Dependency Resolution](dependency-resolution.md) — what users see when
  building and installing.
- [ADR-0016: Advisory runtime dependencies](../adr/0016-advisory-runtime-dependencies.md)
  — registry-level decision (no FK enforcement, soft TEXT deps).
- [ADR-0017: Plan source as single dep truth + ELF lint](../adr/0017-plan-source-single-dep-truth.md)
  — package-time lint policy and why ELF data never round-trips into the
  registry.

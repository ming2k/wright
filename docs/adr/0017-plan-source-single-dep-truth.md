# ADR-0017: Plan Source as Single Dependency Truth + ELF Lint

## Status

Accepted

## Context

[ADR-0016](0016-advisory-runtime-dependencies.md) shifted the registry to an
advisory model: `dependencies.depends_on` is a soft `plan:output` pointer,
unsatisfied targets surface as diagnostics rather than blocking errors. That
ADR did not say where the runtime-dependency *list itself* comes from.

A natural follow-up was to derive runtime deps automatically from the
produced binary's `DT_NEEDED` entries — the dynamic linker's own list of
shared libraries the program needs at startup. That information is more
accurate than a hand-maintained list (no drift, no forgotten entries, no
stale entries from removed code).

A draft of this approach proposed scanning ELFs at the package step,
computing the union of *declared* `runtime_deps` and *empirical* `DT_NEEDED`
mappings, and writing that union into `.PARTINFO`. The installed registry
would then reflect the union.

Review surfaced a load-bearing problem: the union breaks the invariant that
`plan source = PARTINFO = db`. The plan declares `[A, B]`; the package
ships `[A, B, C]`; the db records `[A, B, C]`. After this point:

- `wright info <part>` shows entries the plan author never wrote, with no
  way to trace them back to a declaration.
- The plan source can no longer be reconstructed from a part archive
  (round-trip is lossy).
- Authors have no incentive to keep `runtime_deps` updated — the system
  fixes silently. The plan source rots into ornament.
- Different toolchain versions or `--as-needed` linker behaviour produce
  different `DT_NEEDED` sets for byte-identical source. Build
  reproducibility now depends on the host toolchain, not just the plan.
- The file you edit and the data the system uses are no longer the same
  thing. Debuggability collapses.

The drift cost outweighs the accuracy benefit. ELF data is valuable for
*verification*, not for *augmentation*.

## Decision

The plan source (`runtime_deps` per output) is the **single source of
truth** for runtime dependencies. PARTINFO and the installed database
record exactly what the plan declared; nothing is added, nothing is
inferred.

ELF `DT_NEEDED` scanning is added as a **package-time lint** that compares
the declared set against the empirical set and reports mismatches. It
never writes data into PARTINFO.

### Policy: direction-C

Two directions of mismatch carry different meaning:

| Direction | Meaning | Action |
|-----------|---------|--------|
| **ELF needs `X`, plan does not declare `X`** | The author forgot a declaration. The binary will fail to start without `X`. | **Error.** Package step fails. Author must add `X` to `runtime_deps` (or to `link_deps` if `X` is not yet a known plan, then declare it). |
| **Plan declares `Y`, ELF does not need `Y`** | Likely a dlopen target, data-file dependency, or stale declaration. The author may have a legitimate reason. | **Not reported during packaging.** These surface globally via `wright doctor` after the full dependency closure is available. |
| **ELF needs `Z`, no part provides SONAME `Z`** | The binary links a library wright cannot account for — vendored, host-provided, or genuinely missing. | **Not reported during packaging.** These surface globally via `wright doctor` after the full dependency closure is available. |

The asymmetry is deliberate. Forgotten declarations are *silent footguns*
at runtime; they deserve a hard stop at build time. Surplus declarations
and unmapped SONAMEs are merely noise during batch builds (the dependency
closure is often incomplete at package time). They are surfaced globally
by `wright doctor`, which scans the entire archive collection with a
complete SONAME index.

### What is scanned

For each output of the plan being packaged:

1. Walk the staging tree for files that parse as ELF.
2. For each ELF file, extract `DT_NEEDED` entries.
3. Filter out SONAMEs provided by this part itself (multi-output plans
   often link their own libraries).
4. Resolve each remaining SONAME against the SONAME → part index built
   from this plan's link-deps closure (and the installed db, for parts
   already on disk).
5. Compare resolved part set against the output's declared
   `runtime_deps`. Apply the policy table above.

### What is *not* scanned

- dlopen-loaded libraries are invisible to `DT_NEEDED`. They remain the
  author's responsibility to declare manually.
- Data files, configuration files, and runtime-discovered resources are
  outside ELF entirely. They remain the author's responsibility.
- The lint operates per-output. It does not attempt to reason across
  multiple plan invocations or to second-guess `link_deps`.

### Where the lint lives

The check runs in the package step (after build, before PARTINFO is
finalized and the archive is sealed). Failure of the lint fails the
package step the same way any other validation does. Workflow resume
machinery handles this naturally — the user fixes the plan and re-runs.

A future `wright lint --elf` could surface the same check on existing
plan/staging trees outside a full build, but that is not required for
this ADR.

### What does not change

- `dependencies` schema is unchanged from ADR-0016: still
  `(part_id, depends_on TEXT, version_constraint TEXT?)`, still soft.
- `link_deps` and `build_deps` continue to live in plan source for build
  scheduling. They are not persisted to the registry (per ADR-0016).
- `wright check` and the advisory model from ADR-0016 are unchanged. The
  ELF lint operates earlier in the pipeline (build-time), not at the
  registry layer.

## Alternatives considered

**Auto-augment PARTINFO with ELF-derived deps.** Rejected — see Context.
Breaks plan/PARTINFO/db consistency, undermines reproducibility, rots
plan source.

**ELF as warn-only, no error.** Considered. Provides a friendly nudge
but leaves the silent-footgun case (forgotten declaration → broken
binary) unrecovered. Authors ignore warnings under deadline pressure.
The cost of the strict error is one extra plan edit per linker change,
which is the same edit ELF would have done silently anyway — except now
it lives in the source where future readers can see it.

**Two separate fields (`dlopen_deps`, autogenerated `runtime_deps`).**
Rejected. Two fields with overlapping semantics confuse plan authors,
and the autogenerated field still produces the source/db drift this
ADR exists to avoid.

**No ELF scanning at all (pure manual + `wright launch` ldd
preflight).** Defensible. The user-facing safety net is similar — broken
declarations surface either at package time (with ELF lint) or at launch
time (with ldd preflight). Choosing ELF lint moves the surface earlier
in the pipeline (where it's cheapest to fix) and gives feedback in CI
rather than only on the deploy host. Both are coherent; this ADR picks
the earlier-feedback option.

## Consequences

### Positive

- `plan source = PARTINFO = db` invariant restored and preserved.
- Forgotten `runtime_deps` declarations surface at build time, not at
  the user's first `wright launch`.
- Reverse-rebuild logic, info display, and round-tripping all stay
  honest: what the plan says is what the system records.
- Build reproducibility is unaffected by the lint — the lint's output
  is pass/fail/warn, not data injection.
- Plan authors retain full ownership of the `runtime_deps` list, which
  matches the philosophical position that wright is a build orchestrator,
  not a magic-fixing package manager.

### Negative

- Plan authors must maintain `runtime_deps` accurately. When code
  changes link relationships (e.g. `-lfoo` added or removed), the
  `runtime_deps` list must be edited in the same change.
- Genuine dlopen / data-file deps are no longer visible during
  packaging; authors must run `wright doctor` after batch installs to
  detect stale declarations.
- The package step depends on a SONAME → part index. Building that
  index requires a non-trivial walk of the link-deps closure.

## Known v0 limitations

- **SONAME index uses filename-basename heuristic.** A library's real
  `DT_SONAME` may differ from its on-disk filename (versioned chains
  like `libfoo.so` → `libfoo.so.3` → `libfoo.so.3.2` may surprise).
  Promoting to real `DT_SONAME` extraction requires per-archive
  decompression of every `.so`; not justified until false-negative
  evidence accumulates. Misses surface as `unmapped` warnings, not
  silent skips.
- **Index is rebuilt every package run.** No cache. Acceptable at
  current scale; revisit if package time becomes dominated by index
  building.

## Migration

- Add ELF parser (`src/part/elf.rs`) using `goblin`.
- Add SONAME → part resolver near the package step.
- Wire the lint into the package step before PARTINFO write.
- No schema change. No database migration.
- Existing plans with under-declared `runtime_deps` will fail their next
  package build. The fix is mechanical: add the missing entries the
  lint reports.

## Related

- [ADR-0016](0016-advisory-runtime-dependencies.md) — the registry-level
  advisory model this ADR sits on top of.
- [Dependency Philosophy](../explanation/dependency-philosophy.md) —
  user-facing exposition; will be updated to mention the ELF lint.

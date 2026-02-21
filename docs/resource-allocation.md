# Resource Allocation

This page explains how `wbuild` allocates CPU time across builds and how to
tune each layer.

## Three Layers of Parallelism

| Layer | Controls | Where to configure |
|-------|----------|--------------------|
| **Worker concurrency** | How many packages build simultaneously | `-w` / `--workers` on the CLI |
| **NPROC modifier** | Per-plan thread profile (halve, force serial, …) | `build_type` in `plan.toml` |
| **NPROC cap** | Hard ceiling on compiler threads | `jobs` in `plan.toml` (per-plan) or `wright.toml` (global) |

The layers compose. On a 16-core machine with `-w 4` and the default profile,
4 packages run concurrently each with 4 compiler threads — 16 threads total.
A plan declaring `build_type = "heavy"` in that same run receives 2 threads
instead of 4.

## Worker Concurrency (`-w`)

The scheduler runs as many workers as `-w` allows, but only launches a package
when **all of its dependencies in the current build set have finished**.
Dependency ordering is enforced automatically; `-w` is a ceiling, not a
guarantee.

```
dependency graph:          with -w 3:
  A ─┐
  B ─┼─► D ─► F          step 1: A, B, C  (no deps, all launch)
  C ─┘                    step 2: D        (waits for A, B)
                           step 3: E        (waits for C)
  C ──► E                 step 4: F        (waits for D)
```

Setting `-w` higher than the number of packages independent at any given point
has no effect — the scheduler finds no additional ready work.

## Compiler Parallelism (`$NPROC`)

Wright injects `$NPROC` and tool-specific equivalents into every sandboxed
build stage so scripts do not need to hard-code thread counts:

| Variable | Picked up by |
|----------|-------------|
| `$NPROC` | Shell scripts (`make -j${NPROC}`, `ninja -j${NPROC}`) |
| `MAKEFLAGS=-j<N>` | GNU Make (automatic) |
| `CMAKE_BUILD_PARALLEL_LEVEL=<N>` | CMake (`cmake --build`) |
| `CARGO_BUILD_JOBS=<N>` | Cargo |

These are always injected from the resolved `$NPROC` value, regardless of
`build_type`.

## Build Types (`build_type`)

`build_type` declares the resource profile of a plan.  It tells the scheduler
*why* a thread limit is needed so it can choose the right value automatically,
rather than forcing plan authors to hard-code a number.

| `build_type` | `$NPROC` | Extra env injected | Notes |
|--------------|----------|--------------------|-------|
| `"default"` *(default)* | scheduler share | — | Standard parallel builds |
| `"make"` | scheduler share | — | Semantic alias for `default`; use to signal a make-based build |
| `"rust"` | scheduler share | — | Semantic alias for `default`; `CARGO_BUILD_JOBS` is already injected from NPROC |
| `"go"` | scheduler share | `GOFLAGS=-p=<N>`, `GOMAXPROCS=<N>` | Bounds Go's runtime and build parallelism |
| `"heavy"` | scheduler share ÷ 2 (min 1) | — | RAM-bound builds: Rust+LTO, JVM, large C++ |
| `"serial"` | 1 | — | Builds that cannot parallelize at all |
| `"custom"` | scheduler share | — | Semantic alias for `default`; pair with `[options.env]` |

`"make"`, `"rust"`, and `"custom"` are **semantic aliases** — they carry the
same NPROC behaviour as `"default"` and exist to communicate intent in the
plan file, not to change scheduling.

```toml
[options]
build_type = "heavy"
jobs = 4          # additional hard cap: never exceed 4 threads
```

## Per-plan Thread Cap (`jobs`)

`jobs` in `plan.toml` is a per-plan absolute ceiling applied *after* the
`build_type` modifier:

```
effective NPROC = min(after_type_modifier, plan_jobs)
```

It is independent of the global `[build] jobs` in `wright.toml`.  Leave it
unset (or set to 0) for no extra cap.

Example: `build_type = "heavy"` + `jobs = 4` on a 16-core machine with
`-w 4` → `min(16/4 / 2, 4)` = `min(2, 4)` = **2**.

## Package-level Environment (`[options.env]`)

Environment variables declared here are injected into **every** lifecycle stage
of the plan:

```toml
[options]
build_type = "rust"
env = { RUSTFLAGS = "-C target-cpu=native", RUST_BACKTRACE = "1" }
```

**Priority and substitution behaviour:**

- Package-level `env` is available as both **process environment** and
  **`${VAR}` script substitution**.
- Per-stage `[lifecycle.<stage>.env]` overrides package-level env in the
  **process environment**, but does NOT affect `${VAR}` substitution in the
  script body.  If a script uses `${CFLAGS}` literally, it gets the
  package-level value even when the stage env also sets `CFLAGS`.  Use
  package-level env for vars referenced via `${}`, and per-stage env for
  vars consumed directly by the build tool from its environment.

## NPROC Resolution

NPROC is computed in four steps at the moment each worker stage launches:

```
1. scheduler share  =  total_cpus / active_workers
2. base             =  global_jobs > 0  ?  min(global_jobs, share)  :  share
3. after_type       =  serial → 1  |  heavy → max(base/2, 1)  |  others → base
4. final NPROC      =  plan_jobs > 0  ?  min(after_type, plan_jobs)  :  after_type
```

`active_workers` is the count of packages currently building at the moment
this stage is launched — not the `-w` ceiling.  When the graph fans out, each
package gets a smaller share; when it collapses to a single runnable package,
that package gets the full CPU budget.

Thread budgets are **locked when a stage starts**.  A stage that is already
running is not re-threaded if another worker finishes mid-flight.

### Example: 16-core machine

| `-w` | `build_type` | global `jobs` | plan `jobs` | NPROC |
|------|-------------|---------------|-------------|-------|
| 0 (→16) | `default` | 0 | — | dynamic: 16 → 8 → 4 … as workers launch |
| 4 | `default` | 0 | — | 4 |
| 4 | `heavy` | 0 | — | 2 |
| 4 | `heavy` | 0 | 4 | 2 (heavy wins; plan cap not binding) |
| 4 | `heavy` | 0 | 1 | 1 (plan cap wins) |
| 4 | `serial` | 0 | — | 1 |
| 1 | `default` | 0 | — | 16 |
| 4 | `default` | 8 | — | 4 (scheduler share is already ≤ global cap) |
| 4 | `default` | 0 | 2 | 2 (plan cap) |

## Tuning for Your Workload

**Default** — works well for typical packages.  Wright auto-detects the CPU
count and divides it across concurrent workers.

**RAM-bound packages** — use `build_type = "heavy"` to halve the thread share
per package.  For a tighter absolute limit, add `jobs = N`.

**Fully sequential builds** — use `build_type = "serial"`.

**System-wide ceiling** — set `[build] jobs` in `wright.toml` to cap all
plans regardless of `build_type`:

```toml
[build]
jobs = 4
```

**I/O-bound builds** (packages that download at configure time) tolerate more
workers than CPUs.  Raise `-w` explicitly — but note that each worker's `$NPROC`
will decrease proportionally since `NPROC = total_cpus / active_workers`:

```bash
wbuild run -w 8 @base   # 8 workers; each gets ~2 threads on a 16-core machine
```

**Single package, maximum throughput** — `-w 0` with one target gives that
one worker all CPU threads as its `$NPROC`.

## Memory and Time Limits

Wright can enforce hard resource limits per build stage via the kernel's
`rlimit` interface.  Off by default.

| Setting | Where | Effect |
|---------|-------|--------|
| `memory_limit` (MB) | `wright.toml [build]` or `plan.toml [options]` | Caps virtual address space (`RLIMIT_AS`). Set generously (2–3× expected peak) — rustc and Go reserve large virtual mappings they never fully commit. |
| `cpu_time_limit` (seconds) | same | Caps aggregate CPU time for the stage process tree (`RLIMIT_CPU`). Kills runaway compiler loops. |
| `timeout` (seconds) | same | Wall-clock deadline per stage. Kills the process group when elapsed, regardless of CPU usage. |

Per-plan values take precedence over global config:

```toml
# wright.toml — global safety nets
[build]
timeout = 7200        # 2-hour wall-clock limit per stage
cpu_time_limit = 3600 # 1-hour CPU-time limit

# plan.toml — tighter limits for a known-fast package
[options]
timeout = 300
memory_limit = 2048
```

# Resource Allocation

This page explains how `wbuild` allocates CPU and memory across builds, and how
to tune the two independent layers of parallelism.

## Two Layers of Parallelism

Wright separates build parallelism into two distinct levels that operate
independently:

| Layer | Controls | Configured by |
|-------|----------|---------------|
| **Worker concurrency** | How many packages build at the same time | `-w` / `--workers` on the CLI |
| **Compiler parallelism** | How many threads each package's build tool spawns | `build_type` in `plan.toml`; `jobs` cap in `wright.toml`; exposed as `$NPROC` in build scripts |

These two values multiply. On a 16-core machine with `-w 4` and the default
`build_type`, up to 4 packages run concurrently, each receiving 4 compiler
threads — 16 threads total, matching the CPU count.

## Worker Concurrency (`-w`)

The scheduler runs as many workers as the `-w` value allows, but only launches
a package when **all of its direct and transitive dependencies in the current
build set have already finished**. Dependency ordering is enforced
automatically; `-w` is a ceiling, not a guarantee.

```
dependency graph:          with -w 3:
  A ─┐
  B ─┼─► D ─► F          step 1: A, B, C  (no deps, all launch)
  C ─┘                    step 2: D        (waits for A, B)
                           step 3: E        (waits for C)
  C ──► E                 step 4: F        (waits for D)
```

Setting `-w` higher than the number of packages that are actually independent
at any given point has no effect — the scheduler simply won't find more
ready-to-run work.

## Compiler Parallelism (`$NPROC`)

Inside each sandboxed build, Wright injects `$NPROC` and the corresponding
tool-specific variables so build scripts don't need to hard-code thread counts:

| Variable | Used by |
|----------|---------|
| `$NPROC` | Shell scripts (`make -j${NPROC}`, `ninja -j${NPROC}`) |
| `MAKEFLAGS=-j<N>` | GNU Make (picked up automatically) |
| `CMAKE_BUILD_PARALLEL_LEVEL=<N>` | CMake (`cmake --build`) |
| `CARGO_BUILD_JOBS=<N>` | Cargo |

## Build Types (`build_type`)

Plans declare a semantic resource profile via `[options] build_type`.  This
replaces the old numeric `jobs` field in `plan.toml` with a label that
expresses *why* a limit is needed, letting the scheduler pick the right value
automatically.

| `build_type` | `$NPROC` | Extra env injected | Typical use |
|--------------|----------|--------------------|-------------|
| `"default"` *(default)* | scheduler share | — | autotools, make, meson, cmake |
| `"make"` | scheduler share | — | explicit make-based builds |
| `"rust"` | scheduler share | — | cargo (CARGO_BUILD_JOBS auto-injected from NPROC) |
| `"go"` | scheduler share | `GOFLAGS=-p=<N>`, `GOMAXPROCS=<N>` | Go toolchain |
| `"heavy"` | scheduler share ÷ 2 | — | Rust+LTO, JVM, large C++ (RAM-bound) |
| `"serial"` | 1 | — | builds that cannot parallelize |
| `"custom"` | scheduler share | — | use `[options.env]` to manage everything |

```toml
# plan.toml — declare the build profile
[options]
build_type = "heavy"
```

## Package-level Environment (`[options.env]`)

Environment variables can be injected into every lifecycle stage of a plan
without repeating them per-stage:

```toml
[options]
build_type = "rust"
env = { RUSTFLAGS = "-C target-cpu=native", RUST_BACKTRACE = "1" }
```

Per-stage `[lifecycle.<stage>.env]` overrides package-level env when both
define the same key.

## NPROC Resolution

When a build starts, the effective `$NPROC` for that worker is resolved in this
order:

```
1. build_type modifier              → serial → 1; heavy → base/2; others → base
2. wright.toml [build] jobs = N     → system-wide ceiling (0 = no cap)
3. scheduler dynamic share          → total_cpus / active_workers at launch time
   final base = jobs cap applied to scheduler share (or scheduler share if jobs=0)
```

`active_workers` is the number of build workers currently running at launch
time. This gives a dynamic isolation model: when only one package is runnable
it can use more threads, and when the graph fans out each package receives a
smaller share.

Note: thread budgets are applied when each stage starts. Already-running stages
are not dynamically re-threaded mid-flight.

### Example: 16-core machine

| `-w` | `build_type` | `jobs` (wright.toml) | NPROC per worker at launch |
|------|-------------|----------------------|---------------------------|
| 0 (auto → 16) | `default` | 0 (auto) | dynamic: 16, 8, 5, … as workers start |
| 4 | `default` | 0 (auto) | 4 |
| 4 | `heavy` | 0 (auto) | 2 (halved) |
| 4 | `serial` | 0 (auto) | 1 |
| 1 | `default` | 0 (auto) | 16 |
| 4 | `default` | 8 (explicit) | 4 (clamped by 16/4) |

## Tuning for Your Workload

**Default (`-w 0`, `build_type = "default"`)** works well for typical package
builds. Wright auto-detects CPU count and divides it across workers.

**Memory-heavy packages** (Rust with LTO, JVM, large C++ codebases) — declare
`build_type = "heavy"` in the plan to halve the compiler thread count and
avoid RAM exhaustion under parallel workers.

**System-wide thread cap** — set `jobs` in `wright.toml` to impose a ceiling
on all plans regardless of `build_type`:

```toml
# wright.toml — limit compiler threads system-wide
[build]
jobs = 4
```

**I/O-bound builds** (packages that download at configure time) can often run
more workers than CPUs without contention. Raise `-w` explicitly:

```bash
wbuild run -w 8 @base
```

**Single package, maximum throughput** — the default `-w 0` with one target
already does this: one worker gets all CPUs as its `$NPROC`.

## Memory and Time Limits

In addition to CPU allocation, Wright can enforce hard resource limits per
build stage via the kernel's `rlimit` interface. These are off by default.

| Setting | Where | Effect |
|---------|-------|--------|
| `memory_limit` (MB) | `wright.toml` `[build]` or `plan.toml` `[options]` | Caps virtual address space (`RLIMIT_AS`). Set generously (2-3× expected usage) — compilers like rustc and Go reserve large virtual mappings they never fully use. |
| `cpu_time_limit` (seconds) | same | Caps aggregate CPU time consumed by the stage process tree (`RLIMIT_CPU`). Kills runaway compiler loops. |
| `timeout` (seconds) | same | Wall-clock deadline per stage. Kills the process group when the deadline passes regardless of CPU usage. |

Per-plan values take precedence over global config. Limits can be mixed:

```toml
# wright.toml — sensible global safety nets
[build]
timeout = 7200        # 2 hour wall-clock limit for any stage
cpu_time_limit = 3600 # 1 hour CPU-time limit

# plan.toml — tighter limits for a known-fast package
[options]
timeout = 300
memory_limit = 2048
```

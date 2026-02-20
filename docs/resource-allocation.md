# Resource Allocation

This page explains how `wbuild` allocates CPU and memory across builds, and how
to tune the two independent layers of parallelism.

## Two Layers of Parallelism

Wright separates build parallelism into two distinct levels that operate
independently:

| Layer | Controls | Configured by |
|-------|----------|---------------|
| **Worker concurrency** | How many packages build at the same time | `-w` / `--workers` on the CLI |
| **Compiler parallelism** | How many threads each package's build tool spawns | `jobs` in `wright.toml` or `plan.toml`; exposed as `$NPROC` in build scripts |

These two values multiply. On a 16-core machine with `-w 4` and `jobs = 4`,
up to 4 packages run concurrently, each using 4 compiler threads — 16 threads
total, matching the CPU count.

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

## NPROC Resolution

When a build starts, the effective `$NPROC` for that worker is resolved in this
order:

```
1. plan.toml [options] jobs = N      → use N exactly
2. wright.toml [build] jobs = N      → use N exactly
3. both auto (jobs = 0 / unset)      → total_cpus / actual_workers
```

Steps 1 and 2 are explicit overrides — the user has expressed intent, so Wright
uses the value as-is. Step 3 is the fully-automatic path: Wright divides the
total CPU count evenly across the active worker pool so that the aggregate
compiler load matches the hardware.

### Example: 16-core machine

| `-w` | `jobs` | NPROC per worker | Total threads |
|------|--------|-----------------|---------------|
| 0 (auto → 16) | 0 (auto) | 1 | 16 |
| 4 | 0 (auto) | 4 | 16 |
| 1 | 0 (auto) | 16 | 16 |
| 4 | 8 (explicit) | 8 | up to 32 (oversubscribed — intentional) |

The last row is oversubscribed. Wright allows it because an explicit `jobs`
value is a deliberate choice, often used when builds are I/O-bound or when
memory pressure matters more than raw throughput.

## Tuning for Your Workload

**Default (`-w 0`, `jobs = 0`)** works well for typical package builds. Wright
auto-detects CPU count and divides it across workers.

**Memory-heavy packages** (Rust, JVM, Go, LTO builds) may require fewer
concurrent compiler threads than CPUs to stay within available RAM. Lower
`jobs` globally or per-plan:

```toml
# wright.toml — limit compiler threads system-wide
[build]
jobs = 4
```

```toml
# plan.toml — limit for one heavy package only
[options]
jobs = 2
```

**I/O-bound builds** (e.g. packages that download at configure time) can often
run more workers than CPUs without contention. Raise `-w` explicitly:

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

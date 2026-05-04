# How to Handle Circular Dependencies

Some parts require themselves or each other to build (e.g., a compiler that compiles itself). Wright resolves this with a two-pass build:

1. **MVP pass** — build the part with a reduced dependency set (no cyclic dep)
2. **Full pass** — rebuild with all dependencies, now that the cycle is broken

## Declare an MVP Phase

Place a `mvp.toml` file next to `plan.toml` with MVP-specific dependencies so the graph becomes acyclic:

```text
gcc/
├── plan.toml
└── mvp.toml
```

`plan.toml`:

```toml
name  = "gcc"
version = "14.2.0"
# ...

build_deps = ["binutils:default", "glibc:default", "gcc:default"]  # gcc needs itself — cycle!
```

`mvp.toml`:

```toml
build_deps = ["binutils:default", "glibc:default"]     # MVP: build without gcc in deps
```

Wright detects the cycle automatically and schedules:

```
INFO Build batch 1/2: bootstrap gcc, build binutils.  ← first pass, no gcc dep
INFO Build batch 2/2: full rebuild gcc.               ← second pass, full deps
```

## Test the MVP Pass Explicitly

To test the MVP pass without a cycle present:

```bash
wright build gcc --mvp
```

## Inspect Cycles Without Building

To see what cycles exist and which parts are MVP candidates:

```bash
wright lint gcc binutils glibc
```

## Dependency Type Classification Comes First

Most apparent cycles are caused by incorrect dependency classification. Before defining phase-specific dependencies, verify that:

- **`link_deps`** is only used for shared libraries your binary actually links against at build time.
- **`runtime_deps`** is used for plugins, loaders, and tools called at runtime.

Reserve phase-specific dependencies for cycles that remain after dependency types are correct.

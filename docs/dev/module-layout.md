# Module Layout

```text
.
├── Cargo.toml                         # workspace and the `wright` package
├── crates/
│   ├── wright-actions/                # application use cases and physical actions
│   │   └── src/
│   │       ├── operations/            # command use cases (build, install, upgrade, launch, …)
│   │       ├── foundry/               # charge -> forge -> mold build pipeline
│   │       ├── action/                # action atoms and their execution handlers
│   │       ├── graph/                 # action-graph construction
│   │       ├── seal/                  # sealing and output slicing
│   │       ├── transaction/           # deploy, upgrade, and remove
│   │       ├── ledger/                # build records and ledger paths
│   │       ├── query/                 # system analysis
│   │       └── util/                  # output, filesystem, logging utilities
│   ├── wright-scheduler/              # Action DAG, planner, concurrent executor
│   ├── wright-resolve/                # target resolution, Package DAG, build waves
│   ├── wright-registry/               # installed index and delivery transactions
│   │   └── migrations/                # immutable schema migrations
│   ├── wright-cache/                  # closure-fingerprint build artefact cache
│   ├── wright-ledger/                 # append-only build ledger and plan snapshots
│   ├── wright-lock/                   # advisory cross-process file locks
│   ├── wright-sandbox/                # namespaces, OverlayFS, resource limits
│   ├── wright-config/                 # configuration schemas and loaders
│   ├── wright-part/                   # archives, folios, stores, FHS/ELF/ABI checks
│   ├── wright-plan/                   # plan parsing, linting, and discovery
│   └── wright-model/                  # dependency-free domain values
└── src/
    ├── bin/
    │   └── wright.rs                  # CLI entry point
    ├── lib.rs                         # compatibility facade
    └── cli/                           # clap schemas and per-command adapters
```

The dependency direction is:

```text
wright           -> wright-{actions,cache,config,ledger,lock,part,plan,registry,resolve,sandbox,scheduler}
wright-actions   -> wright-{cache,config,ledger,lock,model,part,plan,registry,resolve,sandbox,scheduler}
wright-resolve   -> wright-{config,ledger,model,part,plan,registry,scheduler}
wright-scheduler -> wright-{cache,ledger,model,part,plan,registry,sandbox}
wright-registry  -> wright-{ledger,lock}
wright-part      -> wright-model
wright-plan      -> wright-model
wright-config    -> wright-model
wright-sandbox   -> wright-model
wright-cache     -> (no workspace dependencies)
wright-ledger    -> (no workspace dependencies)
wright-lock      -> (no workspace dependencies)
wright-model     -> (no dependencies)
```

Do not introduce reverse edges. `wright-model` must remain free of I/O and
external dependencies. The root package re-exports established library paths
for compatibility, but internal code imports the crate that owns a type.

## Placement Rules

Choose an owner by responsibility, not by which caller needs the code first.

| Responsibility | Owner | Examples |
|----------------|-------|----------|
| Command syntax and argument translation | root `src/cli/` | clap `Args`, command dispatch, CLI-only enums |
| Command use cases and orchestration | `wright-actions` | build, install, upgrade, launch, seal, deploy |
| Action DAG and physical execution | `wright-scheduler` | `ActionGraph`, `ActionPlanner`, `ActionScheduler` |
| Target and dependency resolution | `wright-resolve` | target expansion, bootstrap passes, build waves |
| Persistent installed state | `wright-registry` | SQLite queries, migrations, delivery transactions |
| Build artefact reuse | `wright-cache` | closure fingerprints, CAS entries |
| Build audit trail | `wright-ledger` | `builds.jsonl`, plan snapshots |
| Process coordination | `wright-lock` | advisory command and database locks |
| Sandbox mechanics | `wright-sandbox` | namespaces, OverlayFS, cgroup limits, cancellation |
| Configuration | `wright-config` | `GlobalConfig`, path defaults, environment overlays |
| Part formats and filesystem validation | `wright-part` | archives, compression, FHS, ELF, SONAME, ABI |
| Plan source interpretation | `wright-plan` | parsing, discovery, variables, static linting |
| Dependency-free domain values | `wright-model` | versions, pipeline stages, isolation policy |

Apply these rules when adding or moving code:

1. Keep clap types in the root package. Translate them into request types
   before entering `wright-actions`.
2. Put a command use case in `wright-actions/src/operations/<command>.rs`.
   Keep dependency algorithms, scheduling, and persistence mechanics in their
   owning subsystem instead of growing the operation adapter.
3. Move a value into `wright-model` only when it is independent of files,
   processes, databases, networks, and serialization frameworks.
4. Add persistence behavior to `wright-registry`; do not expose its SQL pool
   to higher layers.
5. Keep helpers beside their only caller. Add a helper to `util/` only when it
   is application-wide, has a precise name, and has no clearer domain owner.

## Naming and Visibility

- Name domain modules with nouns such as `plan`, `part`, and `registry`. Name
  use-case modules after the command verb, such as `install/` or `clean.rs`.
- Use `lib.rs` only as a crate facade. Use `mod.rs` to define a multi-file
  subsystem, not as a home for unrelated behavior.
- Keep modules and symbols private by default. Re-export only the stable entry
  points that a higher layer needs.
- Avoid new `common`, `helpers`, or `misc` modules. A name that broad usually
  means the code has not been assigned to its owner yet.
- Preserve one direction across boundaries. A lower crate must not depend on
  `wright-actions` or on the root `wright` package to reuse a convenience
  helper.

The execution path is intentionally thin at the top:

```text
src/bin/wright.rs -> src/cli/mod.rs::dispatch -> src/cli/<cmd>::run -> wright-actions
```

- `src/bin/wright.rs` detects the internal isolation-helper invocation before
  creating a Tokio runtime; normal invocations load configuration, initialize
  logging, and dispatch.
- `src/cli/<cmd>.rs` owns the clap `Args` and `run` adapter for one command.
  The adapter translates arguments into an application request. See
  [ADR-0020](../adr/0020-merge-cli-and-commands-directories.md).
- `src/cli/common.rs` owns the per-invocation CLI context and shared clap
  enums.
- `crates/wright-actions/src/operations/` owns use cases such as install and
  launch. Operations accept request types, not clap types.
- `crates/wright-resolve/` owns target expansion, bootstrap cycle handling,
  and build-wave planning.
- `crates/wright-scheduler/` owns the Action DAG, the planner that lowers a
  package graph into it, and the concurrent executor.
- `crates/wright-actions/src/foundry/` owns source acquisition and build
  execution. `crates/wright-part/` owns archive and output validation
  mechanics.
- `crates/wright-sandbox/` owns sandbox execution. Its direct runner handles
  explicit host execution; its application-side process supervisor owns
  cancellation, wall-clock limits, logging, and capture; and its
  single-threaded helper owns the namespace protocol, setup, and output
  forwarding. See
  [ADR-0028](../adr/0028-single-threaded-isolation-helper.md).

## `build.rs` and the `with_handlers` cfg

`build.rs` `#[path]`-includes `src/cli/mod.rs` to drive `clap_complete` and
`clap_mangen`. Handler bodies reference modules re-exported from
`wright-actions`; those modules do not exist in the build-script crate.
Handlers, dispatch glue, and the CLI `Context` are therefore gated behind
`#[cfg(with_handlers)]`.

`build.rs` emits:

```text
cargo::rustc-cfg=with_handlers
```

When adding a command, keep the clap `Args` unconditional and gate `run`,
`Context` imports, and action imports with `#[cfg(with_handlers)]`.

# Module Layout

```text
.
├── Cargo.toml                         # workspace and wright package
├── crates/
│   ├── wright-engine/                 # application orchestration
│   │   └── src/
│   │       ├── operations/            # command use cases
│   │       ├── resolve/               # dependency and bootstrap DAGs
│   │       ├── foundry/               # build execution
│   │       ├── isolation/             # sandbox execution
│   │       ├── seal/                  # output sealing
│   │       ├── transaction/           # deploy, upgrade, and remove
│   │       ├── query/                 # system analysis
│   │       └── util/                  # application utilities
│   ├── wright-state/                  # SQLite, WAL/CAS, and locks
│   │   └── migrations/                # immutable schema migrations
│   ├── wright-part/                   # archives, stores, FHS, and ELF
│   ├── wright-plan/                   # plan parsing and discovery
│   └── wright-model/                  # dependency-free domain values
└── src/
    ├── bin/
    │   └── wright.rs                  # CLI entry point
    ├── lib.rs                         # compatibility facade
    └── cli/                           # clap schemas and adapters
```

The enforced dependency direction is:

```text
wright        -> wright-engine, wright-part, wright-plan, wright-state
wright-engine -> wright-part, wright-plan, wright-state, wright-model
wright-part   -> wright-model
wright-plan   -> wright-model
wright-state  -> (no workspace dependencies)
wright-model  -> (no dependencies)
```

Do not introduce reverse edges. `wright-model` must remain free of I/O and
external dependencies. The root package re-exports established library paths
for compatibility, but internal code imports the crate that owns a type.

## Placement Rules

Choose an owner by responsibility, not by which caller needs the code first.

| Responsibility | Owner | Examples |
|----------------|-------|----------|
| Command syntax and argument translation | root `src/cli/` | clap `Args`, command dispatch, CLI-only enums |
| Application use cases and orchestration | `wright-engine` | install, resolve, build, package, deploy |
| Persistent installed state | `wright-state` | SQLite queries, migrations, CAS, process locks |
| Part formats and filesystem validation | `wright-part` | archives, compression, FHS, ELF, SONAME |
| Plan source interpretation | `wright-plan` | parsing, discovery, variables, static linting |
| Dependency-free domain values | `wright-model` | versions, dependency syntax, isolation policy |

Apply these rules when adding or moving code:

1. Keep clap types in the root package. Translate them into engine request
   types before entering `wright-engine`.
2. Put a command use case in `wright-engine/src/operations/<command>.rs`.
   Keep dependency algorithms, build execution, and persistence mechanics in
   their owning subsystem instead of growing the operation adapter.
3. Move a value into `wright-model` only when it is independent of files,
   processes, databases, networks, and serialization frameworks.
4. Add persistence behavior to `wright-state`; do not expose its SQL pool to
   higher layers.
5. Keep helpers beside their only caller. Add a helper to `util/` only when it
   is application-wide, has a precise name, and has no clearer domain owner.

## Naming and Visibility

- Name domain modules with nouns such as `plan`, `part`, and `state`. Name
  use-case modules after the command verb, such as `install/` or `prune.rs`.
- Use `lib.rs` only as a crate facade. Use `mod.rs` to define a multi-file
  subsystem, not as a home for unrelated behavior.
- Keep modules and symbols private by default. Re-export only the stable entry
  points that a higher layer needs.
- Avoid new `common`, `helpers`, or `misc` modules. A name that broad usually
  means the code has not been assigned to its owner yet.
- Preserve one direction across boundaries. A lower crate must not depend on
  `wright-engine` or on the root `wright` package to reuse a convenience
  helper.

The execution path is intentionally thin at the top:

```text
src/bin/wright.rs -> src/cli/mod.rs::dispatch -> src/cli/<cmd>::run -> wright-engine
```

- `src/bin/wright.rs` detects the internal isolation-helper invocation before
  creating a Tokio runtime; normal invocations load configuration, initialize
  logging, and dispatch.
- `src/cli/<cmd>.rs` owns the clap `Args` and `run` adapter for one command.
  The adapter translates arguments into an engine request. See
  [ADR-0020](../adr/0020-merge-cli-and-commands-directories.md).
- `src/cli/common.rs` owns the per-invocation CLI context and shared clap
  enums.
- `crates/wright-engine/src/operations/` owns use cases such as install and
  launch. Operations accept application request types, not clap types.
- `crates/wright-engine/src/resolve/` owns dependency expansion, bootstrap
  cycle handling, and build-wave planning.
- `crates/wright-engine/src/foundry/` owns source acquisition and build
  execution. `crates/wright-part/` owns archive and output validation
  mechanics.
- `crates/wright-engine/src/isolation/` owns sandbox execution. Its direct
  runner handles explicit host execution; its application-side process
  supervisor owns cancellation, wall-clock limits, logging, and capture; and
  its single-threaded helper owns the namespace protocol, setup, and output
  forwarding. See
  [ADR-0028](../adr/0028-single-threaded-isolation-helper.md).

## `build.rs` and the `with_handlers` cfg

`build.rs` `#[path]`-includes `src/cli/mod.rs` to drive `clap_complete` and
`clap_mangen`. Handler bodies reference modules re-exported from
`wright-engine`; those modules do not exist in the build-script crate.
Handlers, dispatch glue, and the CLI `Context` are therefore gated behind
`#[cfg(with_handlers)]`.

`build.rs` emits:

```text
cargo::rustc-cfg=with_handlers
```

When adding a command, keep the clap `Args` unconditional and gate `run`,
`Context` imports, and engine imports with `#[cfg(with_handlers)]`.

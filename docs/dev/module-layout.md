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
wright -> wright-engine -> wright-state -> wright-part -> wright-plan -> wright-model
                         -> wright-part  -> wright-plan -> wright-model
                         -> wright-plan  -> wright-model
```

Do not introduce reverse edges. `wright-model` must remain free of I/O and
external dependencies. The root package re-exports established library paths
for compatibility, but internal code imports the crate that owns a type.

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

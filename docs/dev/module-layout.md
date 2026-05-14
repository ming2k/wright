# Module Layout

```text
src/
├── bin/
│   └── wright.rs     # CLI entry point
├── lib.rs            # Library root
├── cli/              # clap schemas + handlers, one file per subcommand
├── config.rs         # global config
├── operations/       # command use cases and batch driving
├── resolve/          # target resolution, dependency graphs, batch planning (step 1: resolve)
├── foundry/          # single-plan build: charge (fetch/verify/extract), forge (prepare/configure/compile/check/staging), mold (slice outputs) (step 2: build)
├── seal/             # output validation, archive creation (step 3: seal)
├── delivery/         # CAS store + WAL crash recovery for the delivery state machine
├── database/         # installed system state and migration layer
├── isolation/        # sandbox isolation
├── part/             # archive format, local part store, pruning, versions, FHS validation
├── plan/             # plan discovery, parsing, and validation
├── query/            # system analysis
├── transaction/      # install / upgrade / remove / verify
└── util/             # helpers (stdin parsing, locking, logging, …)
```

The execution path is intentionally thin at the top:

```text
src/bin/wright.rs -> src/cli/mod.rs::dispatch -> src/cli/<cmd>::run -> library modules
```

- `src/bin/wright.rs` parses args, initializes logging, loads config, and dispatches.
- `src/cli/<cmd>.rs` owns both the clap `Args` struct and the `run` handler
  for that command. The handler builds an operation request and invokes
  `operations::*`. See [ADR-0020](../adr/0020-merge-cli-and-commands-directories.md).
- `src/cli/common.rs` defines the `Context` struct (config, db_path, root_dir,
  verbose, quiet) built once per invocation and passed to every handler, plus
  the `DomainArg` / `MatchPolicyArg` shared clap enums.
- `src/cli/mod.rs` holds the top-level `Cli` / `Commands` enums and the
  `dispatch` function that constructs a `Context` and routes to the matching
  `cli::<cmd>::run`.
- `src/operations/` owns command use cases such as install and launch, and drives batch execution.
- `src/resolve/` owns graph construction, dependency expansion, and build wave planning.
- `src/foundry/` owns execution of one plan's build: source fetching (Charge), forge stages (Forge), and output slicing (Mold).
- `src/seal/` owns output directory validation (FHS, ELF lint) and archive creation. Output slicing is owned by `src/foundry/mold.rs`.

## `build.rs` and the `with_handlers` cfg

`build.rs` `#[path]`-includes `src/cli/mod.rs` to drive `clap_complete` and
`clap_mangen`. The handler bodies in `src/cli/*.rs` reference internal
modules (`crate::operations`, `crate::resolve`, …) that do not exist in
the build-script crate. To keep one file per command without forcing
`build.rs` to compile those references, handlers and the `Context` struct
are gated behind `#[cfg(with_handlers)]`. `build.rs` emits

```text
cargo::rustc-cfg=with_handlers
```

which is set only when Cargo compiles the main crate. When adding a new
command, follow the same pattern: clap `Args` struct unconditional;
`run`, `Context` imports, and any `crate::operations::*` imports gated
with `#[cfg(with_handlers)]`.

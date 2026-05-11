# Module Layout

```text
src/
├── bin/
│   └── wright.rs     # CLI entry point
├── lib.rs            # Library root
├── cli/              # clap schemas grouped by subcommand
├── commands/         # thin CLI adapters grouped by subcommand
├── config.rs         # global config
├── operations/       # command use cases and batch driving
├── resolve/          # target resolution, dependency graphs, batch planning (step 1: resolve)
├── forge/            # single-plan forge: fetch, pipeline execution, output slicing (step 2: forge)
├── seal/             # output validation, archive creation (step 3: seal)
├── delivery/          # CAS store + WAL crash recovery for the delivery state machine
├── database/         # installed system state and migration layer
├── isolation/        # sandbox isolation
├── part/             # archive format, local part store, pruning, versions, FHS validation
├── plan/             # plan discovery, parsing, and validation
├── query/            # system analysis
├── transaction/      # install / upgrade / remove / verify
└── util/             # helpers
```

The execution path is intentionally thin at the top:

```text
src/bin/wright.rs -> src/cli/* -> src/commands/* -> library modules
```

- `src/bin/wright.rs` parses args, initializes logging, loads config, and dispatches.
- `src/cli/` owns clap-facing argument and help-text definitions only.
- `src/commands/` maps parsed args into operation requests and command locks.
- `src/operations/` owns command use cases such as install and launch, and drives batch execution.
- `src/resolve/` owns graph construction, dependency expansion, and build wave planning.
- `src/forge/` owns execution of one plan's pipeline stages and source fetching.
- `src/seal/` owns output validation (FHS, ELF lint) and archive creation.

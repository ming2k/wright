# Module Layout

```text
src/
├── bin/
│   └── wright.rs     # CLI entry point
├── lib.rs            # Library root
├── cli/              # clap schemas grouped by subcommand
├── commands/         # thin CLI adapters grouped by subcommand
├── config.rs         # global config
├── operations/       # command use cases and workflow driving
├── workflow/         # content-addressed DAG runtime, steps, and resume store
├── planning/         # target resolution, dependency graphs, batches, packaging entry points
├── builder/          # single-plan build lifecycle execution
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
- `src/operations/` owns command use cases such as apply and launch.
- `src/workflow/` owns resumable command execution.
- `src/planning/` owns graph construction, dependency expansion, and build wave planning.
- `src/builder/` owns execution of one plan's lifecycle stages.

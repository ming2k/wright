# Module Layout

```text
src/
├── bin/
│   └── wright.rs     # CLI entry point
├── lib.rs            # Library root
├── cli/              # clap schemas grouped by subcommand
├── commands/         # command handlers grouped by subcommand
├── config.rs         # global config and assembly definitions
├── archive/          # archive pruning and resolution logic
├── builder/          # build orchestration and lifecycle execution
├── database/         # unified database layer (installed system + archive catalogue)
├── isolation/        # sandbox isolation
├── part/             # archive format, versions, FHS validation
├── plan/             # plan parsing and validation
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
- `src/commands/` turns parsed args into calls into `builder`, `archive`, `transaction`, and `query`.

pub mod cli;

pub use wright_actions::{
    action, config, error, foundry, graph, identify, isolation, ledger, operations, query, resolve,
    seal, transaction,
};
pub use wright_actions::{
    cli_aborted, cli_action, cli_error, cli_failed, cli_output, cli_span, cli_warn, errln, out,
    outln,
};

pub use wright_cache as cache;
pub use wright_plan as plan;
pub use wright_registry as registry;
pub use wright_registry::database;
pub use wright_sandbox as sandbox;
pub use wright_scheduler as scheduler;

pub mod delivery {
    pub use wright_registry::delivery::*;

    pub mod store {
        pub use wright_cache::*;
    }
}

/// Compatibility facade for part formats and sealing helpers.
pub mod part {
    pub use wright_part::{
        PartError, Result, Version, VersionConstraint, VersionOp, abi, compression, elf, error,
        fhs, folio, soname, store, version,
    };

    pub mod archive {
        pub use wright_actions::seal::{create_part, create_part_with_isolation};
        pub use wright_part::archive::*;
    }

    pub use archive::*;
}

/// Application utilities plus compatibility paths for helpers now owned by
/// lower-level crates.
pub mod util {
    pub use wright_actions::util::{
        checksum, compact_path, display, download, logging, output, progress, sanitize_filename,
        stdin, timing,
    };

    pub mod compress {
        pub use wright_part::compression::*;
    }

    pub mod lock {
        pub use wright_lock::*;
    }
}

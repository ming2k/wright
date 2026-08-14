pub mod cli;

pub use wright_engine::{
    cli_aborted, cli_action, cli_error, cli_failed, cli_output, cli_span, cli_warn, errln, out,
    outln,
};
pub use wright_engine::{
    config, error, foundry, graph, identify, isolation, ledger, operations, query, resolve, seal,
    transaction,
};

/// Compatibility facade for plan parsing and discovery.
pub use wright_plan as plan;

/// Compatibility facade for installed-state database types.
pub use wright_state::database;

/// Compatibility facade for delivery state and content-addressed storage.
pub mod delivery {
    pub use wright_state::delivery::*;

    pub mod store {
        pub use wright_state::cas::*;
    }
}

/// Compatibility facade for part formats and sealing helpers.
pub mod part {
    pub use wright_part::{
        PartError, Result, Version, VersionConstraint, VersionOp, compression, elf, error, fhs,
        folio, soname, store, version,
    };

    pub mod archive {
        pub use wright_engine::seal::{create_part, create_part_with_isolation};
        pub use wright_part::archive::*;
    }

    pub use archive::*;
}

/// Application utilities plus compatibility paths for helpers now owned by
/// lower-level crates.
pub mod util {
    pub use wright_engine::util::{
        checksum, compact_path, display, download, logging, output, progress, sanitize_filename,
        stdin, timing,
    };

    pub mod compress {
        pub use wright_part::compression::*;
    }

    pub mod lock {
        pub use wright_state::lock::*;
    }
}

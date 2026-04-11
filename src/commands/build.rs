use std::io::BufRead;

use anyhow::{Context, Result};

use crate::builder::orchestrator::{self, BuildOptions};
use crate::cli::build::BuildArgs;
use crate::config::GlobalConfig;
use crate::database::Database;

pub fn execute_build(
    args: BuildArgs,
    config: &GlobalConfig,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let _command_lock = crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(&config.general.db_path),
        crate::util::lock::LockIdentity::Command("build"),
        crate::util::lock::LockMode::Exclusive,
    )
    .context("failed to acquire build command lock")?;

    if args.clear_sessions {
        let db = Database::open(&config.general.db_path).context("failed to open database")?;
        let count = db.clear_all_sessions()?;
        tracing::info!("Cleared {} build session(s)", count);
        return Ok(());
    }

    let mut all_targets = args.targets;
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        for line in std::io::stdin().lock().lines() {
            let line = line.context("failed to read target from stdin")?;
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                all_targets.push(trimmed);
            }
        }
    }

    let effective_dockyards = if args.dockyards != 0 {
        args.dockyards
    } else {
        config.build.dockyards
    };

    orchestrator::run_build(
        config,
        all_targets,
        BuildOptions {
            stages: args.stage,
            fetch_only: args.fetch,
            clean: args.clean,
            force: args.force,
            resume: args
                .resume
                .map(|h| if h.is_empty() { None } else { Some(h) }),
            dockyards: effective_dockyards,
            checksum: args.checksum,
            lint: args.lint,
            skip_check: args.skip_check,
            verbose: verbose > 0,
            quiet,
            mvp: args.mvp,
            print_parts: args.print_parts,
            nproc_per_dockyard: config.build.nproc_per_dockyard,
        },
    )?;
    Ok(())
}

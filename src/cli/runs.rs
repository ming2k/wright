use clap::{Args, Subcommand};

pub const RUNS_AFTER_HELP: &str = "\
Examples:
  wright runs                  # list recent runs
  wright runs show <RUN_ID>    # inspect a run's step DAG
  wright runs gc               # delete old workflows

A run is one attempt to drive a workflow to completion. Workflows are
content-addressed by their CLI inputs, so rerunning the same command
resumes automatically; pass --fresh to start over.";

#[derive(Args, Debug, Clone)]
pub struct RunsArgs {
    #[command(subcommand)]
    pub command: Option<RunsCommand>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum RunsCommand {
    /// List recent runs (default).
    List {
        /// Maximum number of rows to show.
        #[arg(long, default_value = "20")]
        limit: i64,
    },
    /// Show a run's step DAG with status and timings.
    Show {
        /// Run id (prefix match accepted).
        run_id: String,
    },
    /// Delete workflows whose runs have all reached a terminal status.
    Gc {
        /// Retention window in days; workflows older than this are removed.
        #[arg(long, default_value = "30")]
        days: i64,
    },
}

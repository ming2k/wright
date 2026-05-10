use clap::Args;

#[derive(Args, Debug)]
pub struct PsArgs {
    /// Show only workflows with steps in this status
    #[arg(long, value_name = "STATUS")]
    pub status: Option<String>,

    /// Show all workflows including completed ones
    #[arg(long)]
    pub all: bool,
}

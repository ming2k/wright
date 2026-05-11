use clap::Args;

#[derive(Args)]
pub struct PruneArgs {
    /// Keep only the latest archive per part name, while preserving installed versions
    #[arg(long)]
    pub latest: bool,

    /// Apply deletions. Without this flag, only prints what would change
    #[arg(long)]
    pub apply: bool,
}

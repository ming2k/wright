use clap::Parser;

#[derive(Parser, Debug, Clone)]
pub struct PruneArgs {
    /// Delete archives that are present on disk but not registered in the inventory DB
    #[arg(long)]
    pub untracked: bool,

    /// Keep only the latest tracked archive per part name, while preserving installed versions
    #[arg(long)]
    pub latest: bool,

    /// Apply deletions. Without this flag, only prints what would change
    #[arg(long)]
    pub apply: bool,
}

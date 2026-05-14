use clap::Args;

#[cfg(with_handlers)]
use crate::config::GlobalConfig;
#[cfg(with_handlers)]
use crate::error::Result;

#[derive(Args)]
pub struct LintArgs {
    /// Plan names or paths to validate (all plans if omitted)
    pub targets: Vec<String>,
    /// Recurse into subdirectories
    #[arg(long, short = 'r')]
    pub recursive: bool,
    /// Verify deployed part file integrity (SHA-256 checksums)
    #[arg(long)]
    pub verify: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: LintArgs, config: &GlobalConfig) -> Result<()> {
    crate::operations::lint::execute_lint(args.targets, args.recursive, args.verify, config).await
}

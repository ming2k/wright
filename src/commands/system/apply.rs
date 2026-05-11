use std::io::BufRead;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::resolve::{DomainArg, MatchPolicyArg};
use crate::config::GlobalConfig;
use crate::operations::apply::{execute_apply, ApplyRequest};
use crate::part::store::LocalPartStore;
use crate::planning::{DependentsMode, MatchPolicy};

pub fn collect_install_args(mut args: Vec<String>) -> Result<Vec<String>> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        for line in std::io::stdin().lock().lines() {
            let line = line.context("failed to read install target from stdin")?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                args.push(trimmed.to_string());
            }
        }
    }
    Ok(args)
}

pub struct ApplyArgs<'a> {
    pub targets: Vec<String>,
    pub deps: Option<DomainArg>,
    pub rdeps: Option<DomainArg>,
    pub match_policies: Vec<MatchPolicyArg>,
    pub depth: Option<usize>,
    pub force: bool,
    pub dry_run: bool,
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub root_dir: &'a Path,
    pub verbose: u8,
    pub quiet: bool,
    pub part_store: &'a LocalPartStore,
}

fn map_resolve_domain(d: DomainArg) -> DependentsMode {
    match d {
        DomainArg::Link => DependentsMode::Link,
        DomainArg::Runtime => DependentsMode::Runtime,
        DomainArg::Build => DependentsMode::Build,
        DomainArg::All => DependentsMode::All,
    }
}

fn map_match_policy(m: MatchPolicyArg) -> MatchPolicy {
    match m {
        MatchPolicyArg::All => MatchPolicy::All,
        MatchPolicyArg::Missing => MatchPolicy::Missing,
        MatchPolicyArg::Outdated => MatchPolicy::Outdated,
        MatchPolicyArg::Installed => MatchPolicy::Installed,
    }
}

pub async fn execute_system_apply(args: ApplyArgs<'_>) -> Result<()> {
    let ApplyArgs {
        targets,
        deps,
        rdeps,
        match_policies,
        depth,
        force,
        dry_run,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
    } = args;

    let targets = collect_install_args(targets)?;
    if targets.is_empty() {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("no targets received from stdin; did the resolve succeed?");
        }
        anyhow::bail!("no targets specified (pass plan names, group names prefixed with '@', or paths as arguments or via stdin)");
    }

    if dry_run {
        println!("Apply plan (dry-run):");
        println!("  targets: {}", targets.join(", "));
        return Ok(());
    }

    execute_apply(ApplyRequest {
        targets,
        deps: deps.map(map_resolve_domain),
        rdeps: rdeps.map(map_resolve_domain),
        match_policies: match_policies.into_iter().map(map_match_policy).collect(),
        depth,
        force,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
    })
    .await
}

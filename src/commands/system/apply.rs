use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::cli::resolve::{DomainArg, MatchPolicyArg};
use crate::commands::workflow_run::{drive_command, DriveOptions};
use crate::config::GlobalConfig;
use crate::part::store::LocalPartStore;
use crate::workflow::builders::build_apply_workflow;

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
    pub fresh: bool,
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

fn map_resolve_domain(d: DomainArg) -> crate::builder::orchestrator::DependentsMode {
    match d {
        DomainArg::Link => crate::builder::orchestrator::DependentsMode::Link,
        DomainArg::Runtime => crate::builder::orchestrator::DependentsMode::Runtime,
        DomainArg::Build => crate::builder::orchestrator::DependentsMode::Build,
        DomainArg::All => crate::builder::orchestrator::DependentsMode::All,
    }
}

fn map_match_policy(m: MatchPolicyArg) -> crate::builder::orchestrator::MatchPolicy {
    match m {
        MatchPolicyArg::All => crate::builder::orchestrator::MatchPolicy::All,
        MatchPolicyArg::Missing => crate::builder::orchestrator::MatchPolicy::Missing,
        MatchPolicyArg::Outdated => crate::builder::orchestrator::MatchPolicy::Outdated,
        MatchPolicyArg::Installed => crate::builder::orchestrator::MatchPolicy::Installed,
    }
}

pub async fn execute_apply(args: ApplyArgs<'_>) -> Result<()> {
    let ApplyArgs {
        targets,
        fresh,
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
        anyhow::bail!("no targets specified (pass plan names/paths as arguments or via stdin)");
    }

    let resolve_opts = crate::builder::orchestrator::ResolveOptions {
        deps: Some(
            deps.map(map_resolve_domain)
                .unwrap_or(crate::builder::orchestrator::DependentsMode::All),
        ),
        rdeps: rdeps.map(map_resolve_domain),
        match_policies: if match_policies.is_empty() {
            vec![crate::builder::orchestrator::MatchPolicy::Outdated]
        } else {
            match_policies.into_iter().map(map_match_policy).collect()
        },
        depth: Some(depth.unwrap_or(0)),
        include_targets: true,
        preserve_targets: force,
    };

    let build_opts = crate::builder::orchestrator::BuildOptions {
        clean: force,
        force,
        verbose: verbose > 0,
        quiet,
        nproc_per_isolation: config.build.nproc_per_isolation,
        ..Default::default()
    };

    let part_store_arc = Arc::new((*part_store).clone());
    let spec = build_apply_workflow(
        Arc::new(config.clone()),
        targets,
        resolve_opts,
        build_opts,
        root_dir.to_path_buf(),
        part_store_arc,
        force,
        false,
    )
    .await
    .map_err(|e| anyhow::anyhow!("apply workflow: {}", e))?;

    if dry_run {
        println!("Apply plan (dry-run):");
        println!(
            "  workflow {} ({} steps)",
            spec.workflow_id.short(),
            spec.steps.len()
        );
        for s in &spec.steps {
            println!("  {:<14} {}", s.kind, s.id.short());
        }
        return Ok(());
    }

    drive_command(
        spec,
        DriveOptions {
            config,
            db_path,
            fresh,
            quiet,
        },
    )
    .await
    .map(|_| ())
}

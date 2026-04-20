use std::io::BufRead;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::archive::resolver::LocalResolver;
use crate::cli::resolve::{DomainArg, MatchPolicyArg};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::transaction;

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

pub fn resolve_install_paths(resolver: &LocalResolver, args: &[String]) -> Result<Vec<PathBuf>> {
    let mut pkg_paths = Vec::new();
    for arg in args {
        let path = PathBuf::from(arg);
        if path.is_file() {
            pkg_paths.push(path);
            continue;
        }

        match resolver.resolve(arg)? {
            Some(resolved) => pkg_paths.push(resolved.path),
            None => anyhow::bail!(
                "'{}' is not a file and could not be resolved from the local archive catalogue",
                arg
            ),
        }
    }
    Ok(pkg_paths)
}

fn part_entries_for_plan(plan_path: &Path, parts_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let manifest = crate::plan::manifest::PlanManifest::from_file(plan_path)?;
    let mut parts = vec![(
        manifest.plan.name.clone(),
        parts_dir.join(manifest.part_filename()),
    )];
    if let Some(crate::plan::manifest::FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
        for (sub_name, sub_pkg) in pkgs {
            if sub_name == &manifest.plan.name {
                continue;
            }
            let sub_manifest = sub_pkg.to_manifest(sub_name, &manifest);
            parts.push((
                sub_name.clone(),
                parts_dir.join(sub_manifest.part_filename()),
            ));
        }
    }
    Ok(parts)
}

struct ApplyContext<'a> {
    config: &'a GlobalConfig,
    installed_db_path: &'a Path,
    resolver: &'a LocalResolver,
    root_dir: &'a Path,
    force: bool,
    verbose: bool,
    quiet: bool,
    dry_run: bool,
}

fn build_options_for_apply(ctx: &ApplyContext) -> crate::builder::orchestrator::BuildOptions {
    crate::builder::orchestrator::BuildOptions {
        // `apply --force` is a whole-pipeline override for the build/install
        // flow. Keep downloaded sources cached, but clear the per-plan
        // workspace and build cache so the build side really rebuilds.
        clean: ctx.force,
        force: ctx.force,
        verbose: ctx.verbose,
        quiet: ctx.quiet,
        print_parts: false,
        nproc_per_isolation: ctx.config.build.nproc_per_isolation,
        ..Default::default()
    }
}

fn map_resolve_domain(
    domain: crate::cli::resolve::DomainArg,
) -> crate::builder::orchestrator::DependentsMode {
    match domain {
        crate::cli::resolve::DomainArg::Link => crate::builder::orchestrator::DependentsMode::Link,
        crate::cli::resolve::DomainArg::Runtime => {
            crate::builder::orchestrator::DependentsMode::Runtime
        }
        crate::cli::resolve::DomainArg::Build => {
            crate::builder::orchestrator::DependentsMode::Build
        }
        crate::cli::resolve::DomainArg::All => crate::builder::orchestrator::DependentsMode::All,
    }
}

fn map_match_policy(
    policy: crate::cli::resolve::MatchPolicyArg,
) -> crate::builder::orchestrator::MatchPolicy {
    match policy {
        crate::cli::resolve::MatchPolicyArg::All => crate::builder::orchestrator::MatchPolicy::All,
        crate::cli::resolve::MatchPolicyArg::Missing => {
            crate::builder::orchestrator::MatchPolicy::Missing
        }
        crate::cli::resolve::MatchPolicyArg::Outdated => {
            crate::builder::orchestrator::MatchPolicy::Outdated
        }
        crate::cli::resolve::MatchPolicyArg::Installed => {
            crate::builder::orchestrator::MatchPolicy::Installed
        }
    }
}

fn resolve_options_for_apply(
    deps: Option<crate::cli::resolve::DomainArg>,
    rdeps: Option<crate::cli::resolve::DomainArg>,
    match_policies: Vec<crate::cli::resolve::MatchPolicyArg>,
    depth: Option<usize>,
    force: bool,
) -> crate::builder::orchestrator::ResolveOptions {
    let deps = deps
        .map(map_resolve_domain)
        .unwrap_or(crate::builder::orchestrator::DependentsMode::All);
    let rdeps = rdeps.map(map_resolve_domain);
    let match_policies = if match_policies.is_empty() {
        vec![crate::builder::orchestrator::MatchPolicy::Outdated]
    } else {
        match_policies.into_iter().map(map_match_policy).collect()
    };

    crate::builder::orchestrator::ResolveOptions {
        deps: Some(deps),
        rdeps,
        match_policies,
        // `apply` is a smart convergence command: when the user does not bound
        // traversal depth, follow the full upstream chain so missing or
        // outdated dependencies can be materialized end-to-end.
        depth: Some(depth.unwrap_or(0)),
        include_targets: true,
        // `apply --force` must still rebuild the user's requested targets even
        // when the default `missing` policy would otherwise filter them out as
        // already installed.
        preserve_targets: force,
    }
}

fn apply_targets(
    ctx: ApplyContext,
    targets: Vec<String>,
    resolve_opts: crate::builder::orchestrator::ResolveOptions,
) -> Result<()> {
    use crate::builder::logging;
    use crate::builder::orchestrator::{
        describe_batch_actions, describe_build_resources, BuildExecutionPlan,
    };

    // Single resolution pass to determine which targets were explicitly requested.
    // This drives install-origin tracking (explicit vs. dependency install).
    let explicit_plan_names =
        crate::builder::orchestrator::resolve_explicit_plan_names(ctx.resolver, &targets)?;

    let install_nodeps = resolve_opts.deps.is_none();

    let build_set =
        crate::builder::orchestrator::resolve_build_set(ctx.config, targets, resolve_opts)?;

    if build_set.is_empty() {
        if ctx.dry_run {
            println!(
                "Apply plan (dry-run): already converged. All requested targets match the plan state."
            );
        } else if !ctx.quiet {
            tracing::info!(
                "Apply is already converged; requested targets and their dependencies are already up-to-date. Use --force to rebuild anyway."
            );
        }
        return Ok(());
    }

    let build_opts = build_options_for_apply(&ctx);
    let plan =
        crate::builder::orchestrator::create_execution_plan(ctx.config, build_set, &build_opts)?;

    if ctx.dry_run {
        println!("Apply plan (dry-run):");
        for (batch_idx, tasks) in plan.batches().iter().enumerate() {
            for task in tasks {
                let base = BuildExecutionPlan::task_base_name(task);
                let origin = if explicit_plan_names.contains(base) {
                    "explicit"
                } else {
                    "dep"
                };
                println!(
                    "  batch {}  {}  ({})",
                    batch_idx + 1,
                    plan.describe_task(task, &build_opts),
                    origin
                );
            }
        }
        return Ok(());
    }

    // Track successfully installed parts so we can report partial-apply state on failure.
    let mut applied_parts: Vec<String> = Vec::new();

    if !ctx.quiet {
        let resources = crate::builder::orchestrator::summarize_build_resources(ctx.config);
        tracing::info!("{}", describe_build_resources(resources));
    }

    for (batch_idx, tasks) in plan.batches().iter().enumerate() {
        if !ctx.quiet {
            tracing::info!(
                "{}",
                logging::describe_batch(
                    "Apply",
                    batch_idx + 1,
                    plan.batches().len(),
                    &describe_batch_actions(&plan, tasks, &build_opts),
                ),
            );
        }

        plan.execute_batch(ctx.config, batch_idx, &build_opts)
            .inspect_err(|_| {
                emit_partial_apply_note(&applied_parts);
            })?;

        let mut parts = Vec::new();
        let mut explicit_targets = HashSet::new();
        let mut batch_part_names = Vec::new();
        // Deduplicate part paths within a batch; multi-fabricate plans produce
        // multiple sub-parts from the same part file.
        let mut seen_paths = HashSet::new();

        for task in tasks {
            let base = BuildExecutionPlan::task_base_name(task);
            let plan_path = plan
                .plan_path_for_task(task)
                .context("missing plan path for batch task")?;
            for (part_name, part_path) in
                part_entries_for_plan(plan_path, &ctx.config.general.parts_dir)?
            {
                if !part_path.exists() {
                    emit_partial_apply_note(&applied_parts);
                    anyhow::bail!("expected part was not produced: {}", part_path.display());
                }
                if seen_paths.insert(part_path.clone()) {
                    parts.push(part_path);
                }
                if explicit_plan_names.contains(base) {
                    explicit_targets.insert(part_name.clone());
                }
                batch_part_names.push(part_name);
            }
        }

        let db = InstalledDb::open(ctx.installed_db_path)
            .context("failed to open database for install")?;

        transaction::install_parts_with_explicit_targets(
            &db,
            &parts,
            &explicit_targets,
            ctx.root_dir,
            ctx.resolver,
            ctx.force,
            install_nodeps,
        )
        .inspect_err(|_| {
            emit_partial_apply_note(&applied_parts);
        })?;

        applied_parts.extend(batch_part_names);
    }
    Ok(())
}

fn emit_partial_apply_note(applied_parts: &[String]) {
    if !applied_parts.is_empty() {
        eprintln!(
            "note: partially applied — already installed in previous batches: {}",
            applied_parts.join(", ")
        );
    }
}


pub fn execute_apply(
    targets: Vec<String>,
    deps: Option<DomainArg>,
    rdeps: Option<DomainArg>,
    match_policies: Vec<MatchPolicyArg>,
    depth: Option<usize>,
    force: bool,
    dry_run: bool,
    config: &GlobalConfig,
    installed_db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
    resolver: &LocalResolver,
) -> Result<()> {
    use std::io::IsTerminal;
    let targets = collect_install_args(targets)?;

    if targets.is_empty() {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("no targets received from stdin; did the resolve succeed?");
        }
        anyhow::bail!("no targets specified (pass plan names/paths as arguments or via stdin)");
    }

    let resolve_opts = resolve_options_for_apply(deps, rdeps, match_policies, depth, force);

    match apply_targets(
        ApplyContext {
            config,
            installed_db_path,
            resolver,
            root_dir,
            force,
            verbose: verbose > 0,
            quiet,
            dry_run,
        },
        targets,
        resolve_opts,
    ) {
        Ok(()) => {
            if !dry_run {
                println!("apply completed successfully");
            }
        }
        Err(e) => {
            eprintln!("error: {:#}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

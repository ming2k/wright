use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::archive::resolver::LocalResolver;
use crate::cli::resolve::{DomainArg, MatchPolicyArg};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::transaction;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApplySessionMeta {
    build_session_hash: String,
    task_fingerprints: BTreeMap<String, String>,
}

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

pub async fn resolve_install_paths(
    resolver: &LocalResolver,
    args: &[String],
) -> Result<Vec<PathBuf>> {
    let mut part_paths = Vec::new();
    for arg in args {
        let path = PathBuf::from(arg);
        if path.is_file() {
            part_paths.push(path);
            continue;
        }

        match resolver.resolve(arg).await? {
            Some(resolved) => part_paths.push(resolved.path),
            None => anyhow::bail!(
                "'{}' is not a file and could not be resolved from the local archive catalogue",
                arg
            ),
        }
    }
    Ok(part_paths)
}

fn part_entries_for_plan(plan_path: &Path, parts_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let manifest = crate::plan::manifest::PlanManifest::from_file(plan_path)?;
    match manifest.outputs {
        Some(crate::plan::manifest::OutputConfig::Multi(ref parts)) => Ok(parts
            .iter()
            .map(|(sub_name, sub_part)| {
                let sub_manifest = sub_part.to_manifest(sub_name, &manifest);
                (sub_name.clone(), parts_dir.join(sub_manifest.part_filename()))
            })
            .collect()),
        _ => Ok(vec![(
            manifest.plan.name.clone(),
            parts_dir.join(manifest.part_filename()),
        )]),
    }
}

struct ApplyContext<'a> {
    config: &'a GlobalConfig,
    db_path: &'a Path,
    resolver: &'a LocalResolver,
    root_dir: &'a Path,
    force: bool,
    verbose: bool,
    quiet: bool,
    dry_run: bool,
}

fn build_options_for_apply(ctx: &ApplyContext) -> crate::builder::orchestrator::BuildOptions {
    crate::builder::orchestrator::BuildOptions {
        clean: ctx.force,
        force: ctx.force,
        verbose: ctx.verbose,
        quiet: ctx.quiet,
        print_parts: false,
        nproc_per_isolation: ctx.config.build.nproc_per_isolation,
        package: true,
        ..Default::default()
    }
}

fn domain_arg_key(domain: Option<DomainArg>) -> &'static str {
    match domain.unwrap_or(DomainArg::All) {
        DomainArg::Link => "link",
        DomainArg::Runtime => "runtime",
        DomainArg::Build => "build",
        DomainArg::All => "all",
    }
}

fn match_policy_key(policy: MatchPolicyArg) -> &'static str {
    match policy {
        MatchPolicyArg::Missing => "missing",
        MatchPolicyArg::Outdated => "outdated",
        MatchPolicyArg::Installed => "installed",
        MatchPolicyArg::All => "all",
    }
}

fn compute_apply_session_hash(
    targets: &[String],
    deps: Option<DomainArg>,
    rdeps: Option<DomainArg>,
    match_policies: &[MatchPolicyArg],
    depth: Option<usize>,
    force: bool,
    root_dir: &Path,
) -> String {
    use sha2::{Digest, Sha256};

    let mut normalized_targets = targets.to_vec();
    normalized_targets.sort();
    normalized_targets.dedup();

    let mut normalized_policies: Vec<&'static str> = if match_policies.is_empty() {
        vec!["outdated"]
    } else {
        match_policies
            .iter()
            .copied()
            .map(match_policy_key)
            .collect()
    };
    normalized_policies.sort();
    normalized_policies.dedup();

    let mut hasher = Sha256::new();
    hasher.update(b"wright-apply-session-v1\n");
    hasher.update(root_dir.to_string_lossy().as_bytes());
    hasher.update(b"\n");
    hasher.update(format!("deps={}\n", domain_arg_key(deps)).as_bytes());
    hasher.update(format!("rdeps={}\n", domain_arg_key(rdeps)).as_bytes());
    hasher.update(format!("depth={:?}\n", depth).as_bytes());
    hasher.update(format!("force={}\n", force).as_bytes());
    for policy in normalized_policies {
        hasher.update(policy.as_bytes());
        hasher.update(b"\n");
    }
    for target in normalized_targets {
        hasher.update(target.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

async fn load_apply_resume_state(
    db: &InstalledDb,
    ctx: &ApplyContext<'_>,
    apply_session_hash: &str,
    plan: &crate::builder::orchestrator::BuildExecutionPlan,
) -> Result<Option<(String, HashSet<String>)>> {
    let Some(session) = db.get_execution_session(apply_session_hash).await? else {
        return Ok(None);
    };

    if session.command_kind != "apply" {
        anyhow::bail!(
            "session {} is for '{}', not 'apply'",
            &session.session_hash[..12.min(session.session_hash.len())],
            session.command_kind
        );
    }

    let metadata_json = session.metadata_json.as_deref().context(format!(
        "apply session {} is missing metadata",
        &session.session_hash[..12.min(session.session_hash.len())]
    ))?;
    let metadata: ApplySessionMeta = serde_json::from_str(metadata_json).context(format!(
        "failed to decode apply session {} metadata",
        &session.session_hash[..12.min(session.session_hash.len())]
    ))?;

    let current_fingerprints = crate::builder::orchestrator::task_fingerprints(plan)?;
    for (task, fingerprint) in current_fingerprints {
        let Some(stored) = metadata.task_fingerprints.get(&task) else {
            anyhow::bail!(
                "apply session {} is stale: task '{}' is not part of the original execution plan",
                &session.session_hash[..12.min(session.session_hash.len())],
                task
            );
        };
        if stored != &fingerprint {
            anyhow::bail!(
                "apply session {} is stale: plan '{}' changed since the original run",
                &session.session_hash[..12.min(session.session_hash.len())],
                task
            );
        }
    }

    let completed = crate::builder::orchestrator::load_completed_build_tasks(
        &db,
        ctx.config,
        &plan,
        session.task_session_hash.as_deref().unwrap_or(apply_session_hash),
        true,
    )
    .await?;

    Ok(Some((metadata.build_session_hash, completed)))
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
        depth: Some(depth.unwrap_or(0)),
        include_targets: true,
        preserve_targets: force,
    }
}

async fn apply_targets(
    ctx: ApplyContext<'_>,
    apply_session_hash: &str,
    resume_requested: bool,
    targets: Vec<String>,
    resolve_opts: crate::builder::orchestrator::ResolveOptions,
) -> Result<()> {
    use crate::builder::logging;
    use crate::builder::orchestrator::{
        describe_batch_actions, describe_build_resources, BuildExecutionPlan,
    };

    let explicit_plan_names =
        crate::builder::orchestrator::resolve_explicit_plan_names(ctx.resolver, &targets)?;

    let install_nodeps = resolve_opts.deps.is_none();

    let build_set: Vec<String> =
        crate::builder::orchestrator::resolve_build_set(ctx.config, targets.clone(), resolve_opts)
            .await?;

    let db = InstalledDb::open(ctx.db_path)
        .await
        .context("failed to open database for apply session")?;

    if build_set.is_empty() {
        if let Some(session) = db.get_execution_session(apply_session_hash).await? {
            if session.command_kind == "apply" {
                if let Some(build_session_hash) = session.task_session_hash.as_deref() {
                    let _ = db.clear_execution_session(build_session_hash).await;
                }
                let _ = db.clear_execution_session(apply_session_hash).await;
            }
        }
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

    let (build_session_hash, mut completed_build_tasks) = if resume_requested {
        match load_apply_resume_state(&db, &ctx, apply_session_hash, &plan).await? {
            Some(state) => {
                if !ctx.quiet {
                    tracing::info!(
                        "resuming apply session {}",
                        &apply_session_hash[..12.min(apply_session_hash.len())]
                    );
                }
                state
            }
            None => {
                tracing::warn!(
                    "no existing apply session {} found, starting fresh apply",
                    &apply_session_hash[..12.min(apply_session_hash.len())]
                );
                let build_hash = crate::builder::orchestrator::compute_build_session_hash(
                    &plan,
                    &build_opts,
                    "apply-build",
                )?;
                (build_hash, HashSet::new())
            }
        }
    } else {
        if let Some(existing) = db.get_execution_session(apply_session_hash).await? {
            if existing.command_kind == "apply" {
                if let Some(build_hash) = existing.task_session_hash.as_deref() {
                    let _ = db.clear_execution_session(build_hash).await;
                }
                let _ = db.clear_execution_session(apply_session_hash).await;
            }
        }
        let build_hash = crate::builder::orchestrator::compute_build_session_hash(
            &plan,
            &build_opts,
            "apply-build",
        )?;
        (build_hash, HashSet::new())
    };

    let metadata_json = serde_json::to_string(&ApplySessionMeta {
        build_session_hash: build_session_hash.clone(),
        task_fingerprints: crate::builder::orchestrator::task_fingerprints(&plan)?,
    })
    .context("failed to encode apply session metadata")?;

    db.ensure_execution_session(
        apply_session_hash,
        "apply",
        Some(&build_session_hash),
        Some(&metadata_json),
    )
    .await?;
    db.ensure_execution_session(
        &build_session_hash,
        "build",
        Some(&build_session_hash),
        None,
    )
    .await?;
    let build_items: Vec<String> = plan
        .batches()
        .iter()
        .flat_map(|batch| batch.iter().cloned())
        .collect();
    db.ensure_execution_session_items(&build_session_hash, &build_items)
        .await?;

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

        plan.execute_batch(
            ctx.config,
            batch_idx,
            &build_opts,
            Some(&build_session_hash),
            &completed_build_tasks,
        )
        .await
        .inspect_err(|_| {
            emit_partial_apply_note(&applied_parts);
        })?;
        completed_build_tasks.extend(tasks.iter().cloned());

        let mut parts = Vec::new();
        let mut explicit_targets = HashSet::new();
        let mut batch_part_names = Vec::new();
        let mut seen_paths = HashSet::new();
        let mut plan_map = HashMap::new();

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
                batch_part_names.push(part_name.clone());
                plan_map.insert(part_name, base.to_string());
            }
        }

        // Remove old parts from the same plans before installing new ones
        let plans_in_batch: HashSet<String> = plan_map.values().cloned().collect();
        for plan_name in &plans_in_batch {
            let old_parts = db.get_parts_by_plan(plan_name).await?;
            for old_part in old_parts {
                if !batch_part_names.contains(&old_part.name) {
                    tracing::info!("Removing old part {} from plan {}", old_part.name, plan_name);
                    if let Err(e) = transaction::remove_part(
                        &db, &old_part.name, ctx.root_dir, true,
                    ).await {
                        tracing::warn!("Failed to remove old part {}: {}", old_part.name, e);
                    }
                }
            }
        }

        transaction::install_parts_with_explicit_targets_and_plan_map(
            &db,
            &parts,
            &explicit_targets,
            ctx.root_dir,
            ctx.resolver,
            ctx.force,
            install_nodeps,
            &plan_map,
        )
        .await
        .inspect_err(|_| {
            emit_partial_apply_note(&applied_parts);
        })?;

        applied_parts.extend(batch_part_names);
    }

    db.clear_execution_session(&build_session_hash).await?;
    db.clear_execution_session(apply_session_hash).await?;

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

pub async fn execute_apply(
    targets: Vec<String>,
    resume: Option<String>,
    deps: Option<DomainArg>,
    rdeps: Option<DomainArg>,
    match_policies: Vec<MatchPolicyArg>,
    depth: Option<usize>,
    force: bool,
    dry_run: bool,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
    resolver: &LocalResolver,
) -> Result<()> {
    let targets = collect_install_args(targets)?;
    let (resume_requested, explicit_resume_hash) = match resume {
        Some(hash) if hash.is_empty() => (true, None),
        Some(hash) => (true, Some(hash)),
        None => (false, None),
    };

    if targets.is_empty() {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("no targets received from stdin; did the resolve succeed?");
        }
        anyhow::bail!("no targets specified (pass plan names/paths as arguments or via stdin)");
    }

    let computed_apply_session_hash = compute_apply_session_hash(
        &targets,
        deps,
        rdeps,
        &match_policies,
        depth,
        force,
        root_dir,
    );
    let apply_session_hash =
        explicit_resume_hash.unwrap_or_else(|| computed_apply_session_hash.clone());
    if resume_requested && apply_session_hash != computed_apply_session_hash {
        anyhow::bail!(
            "apply session hash {} does not match the current targets/scope; rerun with the original apply arguments",
            &apply_session_hash[..12.min(apply_session_hash.len())]
        );
    }

    let resolve_opts = resolve_options_for_apply(deps, rdeps, match_policies, depth, force);

    match apply_targets(
        ApplyContext {
            config,
            db_path,
            resolver,
            root_dir,
            force,
            verbose: verbose > 0,
            quiet,
            dry_run,
        },
        &apply_session_hash,
        resume_requested,
        targets,
        resolve_opts,
    )
    .await
    {
        Ok(()) => {
            if !dry_run {
                println!("apply completed successfully");
            }
        }
        Err(e) => {
            if !dry_run {
                eprintln!(
                    "Apply session saved as {}. Resume with the same apply arguments plus --resume{}.",
                    apply_session_hash,
                    if resume_requested {
                        String::new()
                    } else {
                        format!(" {}", apply_session_hash)
                    }
                );
            }
            eprintln!("error: {:#}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

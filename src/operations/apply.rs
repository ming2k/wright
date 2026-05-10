use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::part::group;
use crate::part::store::LocalPartStore;
use crate::planning::{BuildOptions, DependentsMode, MatchPolicy, ResolveOptions};
use crate::workflow::builders::build_apply_workflow;
use crate::workflow::WorkflowSpec;

pub struct ApplyRequest<'a> {
    pub targets: Vec<String>,
    pub deps: Option<DependentsMode>,
    pub rdeps: Option<DependentsMode>,
    pub match_policies: Vec<MatchPolicy>,
    pub depth: Option<usize>,
    pub force: bool,
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub root_dir: &'a Path,
    pub verbose: u8,
    pub quiet: bool,
    pub part_store: &'a LocalPartStore,
}

pub async fn build_apply_spec(request: ApplyRequest<'_>) -> Result<WorkflowSpec> {
    let ApplyRequest {
        targets,
        deps,
        rdeps,
        match_policies,
        depth,
        force,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
    } = request;

    if targets.is_empty() {
        anyhow::bail!(
            "no targets specified (pass plan names, group names prefixed with '@', or paths as arguments or via stdin)"
        );
    }

    let groups_dirs: Vec<PathBuf> = vec![config.general.groups_dir.clone()];
    let (targets, group_assumes, _group_config) =
        group::expand_group_references(targets, &groups_dirs)?;

    if targets.is_empty() {
        anyhow::bail!("no plans to build after expanding groups");
    }

    register_group_assumptions(db_path, &group_assumes).await?;

    let resolve_opts = ResolveOptions {
        deps: Some(deps.unwrap_or(DependentsMode::All)),
        rdeps,
        match_policies: if match_policies.is_empty() {
            vec![MatchPolicy::Outdated]
        } else {
            match_policies
        },
        depth: Some(depth.unwrap_or(0)),
        include_targets: true,
        preserve_targets: force,
    };

    let build_opts = BuildOptions {
        clean: force,
        force,
        verbose: verbose > 0,
        quiet,
        nproc_per_isolation: config.build.nproc_per_isolation,
        ..Default::default()
    };

    build_apply_workflow(
        Arc::new(config.clone()),
        targets,
        resolve_opts,
        build_opts,
        root_dir.to_path_buf(),
        Arc::new((*part_store).clone()),
        force,
        false,
    )
    .await
    .map_err(|e| anyhow::anyhow!("apply workflow: {}", e))
}

async fn register_group_assumptions(
    db_path: &Path,
    assumptions: &[group::GroupAssume],
) -> Result<()> {
    if assumptions.is_empty() {
        return Ok(());
    }

    let db = InstalledDb::open(db_path)
        .await
        .context("failed to open database for group assumptions")?;
    for assume in assumptions {
        db.assume_part(&assume.name, &assume.version)
            .await
            .with_context(|| format!("failed to assume {}", assume.name))?;
    }
    Ok(())
}

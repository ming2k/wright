use std::collections::HashSet;
use std::path::Path;

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::operations::install::{InstallRequest, execute_install};
use crate::part::store::LocalPartStore;
use crate::plan::discovery::PlanIndex;
use crate::plan::manifest::PlanManifest;
use crate::resolve::{DepDomain, MatchPolicy, ResolveOptions, plan_search_dirs, resolve_build_set};

pub async fn execute_upgrade(
    targets: Vec<String>,
    force: bool,
    depth: Option<usize>,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
    part_store: &LocalPartStore,
) -> Result<()> {
    let mut targets = targets;

    // Handle `all` — find every installed plan that has a newer version available.
    if targets.iter().any(|t| t == "all") {
        targets = find_outdated_plans(config, db_path).await?;
        if targets.is_empty() {
            if !quiet {
                println!("all plans are up to date");
            }
            return Ok(());
        }
        if !quiet {
            println!("found {} plan(s) to upgrade", targets.len());
        }
    } else if !force {
        // For explicit targets without --force, filter out plans that are
        // already up-to-date so we don't waste time rebuilding them.
        targets = filter_outdated_targets(&targets, config, db_path).await?;
        if targets.is_empty() {
            if !quiet {
                println!("specified plans are already up to date");
            }
            return Ok(());
        }
    }

    if targets.is_empty() {
        return Err(WrightError::ForgeError(
            "no targets specified (pass plan names or `all`)".into(),
        ));
    }

    // Resolve build set including link reverse-dependencies.
    // preserve_targets=true so the outdated explicit targets are always rebuilt.
    let resolve_opts = ResolveOptions {
        deps: DepDomain::ALL,
        rdeps: DepDomain::LINK,
        match_policies: vec![MatchPolicy::Outdated],
        depth: Some(depth.unwrap_or(0)),
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(config, targets.clone(), resolve_opts)
        .await
        .map_err(|e| WrightError::ForgeError(format!("resolve upgrade set: {}", e)))?;

    if build_set.is_empty() {
        if !quiet {
            println!("nothing to upgrade");
        }
        return Ok(());
    }

    if !quiet {
        println!("upgrade set: {}", build_set.join(", "));
    }

    // Run the full install workflow (resolve → forge → seal → deploy) for the resolved set.
    execute_install(InstallRequest {
        targets: build_set,
        dep_domain: DepDomain::ALL,
        match_policies: vec![],
        depth,
        force,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
        build_opts: None,
        run_hooks: true,
    })
    .await
}

/// Filter explicit targets to only those whose plan manifest differs from the
/// deployed version.
async fn filter_outdated_targets(
    targets: &[String],
    config: &GlobalConfig,
    db_path: &Path,
) -> Result<Vec<String>> {
    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open database: {}", e)))?;

    let plan_dirs = plan_search_dirs(config);
    let index = PlanIndex::discover(&plan_dirs)?;

    let mut outdated = Vec::new();
    for target in targets {
        let plan_path = match index.path_for(target) {
            Some(p) => p,
            None => {
                // If the plan doesn't exist locally, skip it (it may be an
                // externally-provided part or a typo).
                continue;
            }
        };

        let manifest = match PlanManifest::from_file(plan_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let installed = match db.get_part(&manifest.metadata.name).await {
            Ok(Some(p)) => p,
            _ => {
                // Not installed — nothing to upgrade.
                continue;
            }
        };

        let plan = match db.get_plan_by_id(installed.plan_id).await? {
            Some(p) => p,
            None => continue,
        };

        let plan_epoch = manifest.metadata.epoch as i64;
        let plan_release = manifest.metadata.release as i64;
        let plan_version = manifest.metadata.version.as_deref().unwrap_or("");

        if plan_epoch != plan.epoch || plan_release != plan.release || plan_version != plan.version
        {
            outdated.push(target.clone());
        }
    }

    Ok(outdated)
}

/// Scan every installed part, compare its deployed version with the current
/// plan manifest, and return the names of plans that are newer.
async fn find_outdated_plans(config: &GlobalConfig, db_path: &Path) -> Result<Vec<String>> {
    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open database: {}", e)))?;

    let installed = db
        .list_parts()
        .await
        .map_err(|e| WrightError::DatabaseError(format!("list parts: {}", e)))?;

    let plan_dirs = plan_search_dirs(config);
    let index = PlanIndex::discover(&plan_dirs)?;

    let mut outdated = Vec::new();
    for part in installed {
        let plan_name = &part.plan_name;
        let Some(plan_path) = index.path_for(plan_name) else {
            continue; // no local plan for this installed part
        };

        let manifest = match PlanManifest::from_file(plan_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let installed_epoch = part.epoch;
        let installed_release = part.release;
        let installed_version = &part.version;

        let plan_epoch = manifest.metadata.epoch as i64;
        let plan_release = manifest.metadata.release as i64;
        let plan_version = manifest.metadata.version.as_deref().unwrap_or("");

        if plan_epoch != installed_epoch
            || plan_release != installed_release
            || plan_version != installed_version
        {
            outdated.push(plan_name.clone());
        }
    }

    // De-duplicate while preserving order.
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();
    for name in outdated {
        if seen.insert(name.clone()) {
            deduped.push(name);
        }
    }
    Ok(deduped)
}

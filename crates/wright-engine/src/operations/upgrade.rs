use std::collections::HashSet;
use std::path::Path;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::operations::install::{InstallRequest, execute_install};
use crate::resolve::{
    DepDomain, MatchPolicy, RebuildReason, ResolveOptions, plan_search_dirs, resolve_build_set,
};
use wright_part::store::LocalPartStore;
use wright_plan::discovery::PlanIndex;
use wright_plan::manifest::PlanManifest;
use wright_state::database::InstalledDb;

pub async fn execute_upgrade(
    targets: Vec<String>,
    force: bool,
    dry_run: bool,
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

    if build_set.names.is_empty() {
        if !quiet {
            println!("nothing to upgrade");
        }
        return Ok(());
    }

    let target_set: HashSet<&str> = targets.iter().map(String::as_str).collect();
    let describe_extra = |name: &str| {
        describe_rdep(
            name,
            build_set.rebuild_reasons.get(name),
            build_set.rebuild_triggers.get(name),
        )
    };

    if !quiet {
        let mut extras: Vec<&str> = build_set
            .names
            .iter()
            .map(String::as_str)
            .filter(|n| !target_set.contains(n))
            .collect();
        extras.sort_unstable();
        if !extras.is_empty() {
            let described: Vec<String> = extras.iter().map(|n| describe_extra(n)).collect();
            println!(
                "also upgrading {} reverse {}: {}",
                extras.len(),
                if extras.len() == 1 {
                    "dependency"
                } else {
                    "dependencies"
                },
                described.join(", ")
            );
        }
    }

    if dry_run {
        println!("[dry-run] upgrade -> {}", root_dir.display());
        println!(
            "[dry-run] would rebuild and deploy {} plan(s):",
            build_set.names.len()
        );
        for name in &build_set.names {
            if target_set.contains(name.as_str()) {
                println!("  {}", name);
            } else {
                println!("  {}", describe_extra(name));
            }
        }
        return Ok(());
    }

    // Run the full install workflow (resolve → forge → seal → deploy) for the resolved set.
    execute_install(InstallRequest {
        targets: build_set.names,
        deps: DepDomain::ALL,
        rdeps: DepDomain::empty(),
        match_policies: vec![],
        depth,
        force,
        clean: force,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
        build_opts: None,
        run_hooks: true,
        dry_run: false,
    })
    .await
}

/// Render a reverse dependency pulled into an upgrade together with the
/// reason it is being rebuilt, e.g. `aegis (link-depends on optics)` or
/// `wavora (depends on aegis)`. Falls back to the bare plan name when no
/// trigger was recorded.
fn describe_rdep(name: &str, reason: Option<&RebuildReason>, trigger: Option<&String>) -> String {
    let Some(trigger) = trigger else {
        return name.to_string();
    };
    match reason {
        Some(RebuildReason::LinkDependency) => format!("{name} (link-depends on {trigger})"),
        _ => format!("{name} (depends on {trigger})"),
    }
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

        // Look up the registered plan record (not a part). For multi-output
        // plans the output parts are named after their `[[output]]` entries
        // (e.g. `gcc-libs`, `gcc-dev`) and need not include the plan name
        // itself, so a part lookup keyed on the plan name would miss them and
        // wrongly report the target as not installed. The `plans` table is
        // keyed by plan name and tracks the deployed version regardless of how
        // many outputs the plan produces.
        let plan = match db.get_plan(&manifest.metadata.name).await? {
            Some(p) => p,
            None => {
                // Not installed — nothing to upgrade.
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::{describe_rdep, filter_outdated_targets};
    use crate::config::GlobalConfig;
    use crate::resolve::RebuildReason;
    use wright_state::database::{InstalledDb, NewPart, NewPlan};

    #[test]
    fn describe_rdep_explains_link_and_transitive_triggers() {
        let trigger = "optics".to_string();
        assert_eq!(
            describe_rdep("aegis", Some(&RebuildReason::LinkDependency), Some(&trigger)),
            "aegis (link-depends on optics)"
        );
        assert_eq!(
            describe_rdep("wavora", Some(&RebuildReason::Transitive), Some(&trigger)),
            "wavora (depends on optics)"
        );
        // No recorded trigger (e.g. pulled in for another reason): bare name.
        assert_eq!(describe_rdep("orphan", None, None), "orphan");
    }

    #[tokio::test]
    async fn explicit_multi_output_plan_is_detected_as_outdated() {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        let plan_dir = plans_dir.join("split-plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(
            plan_dir.join("plan.toml"),
            r#"
name = "split-plan"
version = "2.0.0"
release = 1
description = "split plan"
license = "MIT"
arch = "x86_64"

[[output]]
name = "split-runtime"
description = "runtime output"
include = ["/usr/lib/**"]

[[output]]
name = "split-devel"
description = "development output"
include = ["/usr/include/**"]
"#,
        )
        .unwrap();

        let db_path = temp.path().join("wright.db");
        let db = InstalledDb::open(&db_path).await.unwrap();
        let plan_id = db
            .insert_plan(NewPlan {
                name: "split-plan",
                version: "1.0.0",
                release: 1,
                epoch: 0,
                arch: "x86_64",
            })
            .await
            .unwrap();
        for output in ["split-runtime", "split-devel"] {
            db.insert_part(NewPart {
                name: output,
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();
        }
        assert!(db.get_part("split-plan").await.unwrap().is_none());
        drop(db);

        let mut config = GlobalConfig::default();
        config.general.plans_dir = plans_dir;

        let outdated = filter_outdated_targets(&["split-plan".to_string()], &config, &db_path)
            .await
            .unwrap();
        assert_eq!(outdated, ["split-plan"]);
    }
}

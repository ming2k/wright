use std::collections::HashSet;
use std::path::Path;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::operations::install::{InstallRequest, execute_install, manifest_part_names};
use crate::resolve::{
    DepDomain, MatchPolicy, RebuildReason, ResolveOptions, plan_search_dirs, resolve_build_set,
};
use wright_part::store::LocalPartStore;
use wright_plan::discovery::PlanIndex;
use wright_plan::manifest::PlanManifest;
use wright_state::database::{InstalledDb, PlanRecord};

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
                crate::outln!("all plans are up to date");
            }
            return Ok(());
        }
        if !quiet {
            crate::outln!("found {} plan(s) to upgrade", targets.len());
        }
    } else if !force {
        // For explicit targets without --force, filter out plans that are
        // already up-to-date so we don't waste time rebuilding them.
        targets = filter_outdated_targets(&targets, config, db_path).await?;
        if targets.is_empty() {
            if !quiet {
                crate::outln!("specified plans are already up to date");
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
            crate::outln!("nothing to upgrade");
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

    let mut extras: Vec<&str> = build_set
        .names
        .iter()
        .map(String::as_str)
        .filter(|n| !target_set.contains(n))
        .collect();
    extras.sort_unstable();
    if !extras.is_empty() && !quiet {
        let mut groups: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for extra in &extras {
            let trigger = build_set
                .rebuild_triggers
                .get(*extra)
                .map(String::as_str)
                .unwrap_or_else(|| targets.first().map(String::as_str).unwrap_or("target"));
            groups.entry(trigger).or_default().push(extra);
        }

        crate::cli_action!(
            "Cascading",
            "rdeps of {} ({} packages affected):",
            targets.join(", "),
            extras.len()
        );
        for (trigger, pkgs) in groups {
            let label = if target_set.contains(trigger) {
                format!("via {} (direct)", trigger)
            } else {
                format!("via {}", trigger)
            };
            crate::outln!("    {:<24} {}", label, pkgs.join(", "));
        }
    }

    if dry_run {
        crate::outln!("[dry-run] upgrade -> {}", root_dir.display());
        crate::outln!(
            "[dry-run] would rebuild and deploy {} plan(s):",
            build_set.names.len()
        );
        for name in &build_set.names {
            if target_set.contains(name.as_str()) {
                crate::outln!("  {}", name);
            } else {
                crate::outln!("  {}", describe_extra(name));
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

/// Decide whether an installed plan is outdated relative to its current plan
/// manifest, taking the manifest as the source of truth. A plan is outdated
/// when the deployed (epoch, version, release) differs, when the manifest
/// content changed since deploy (the recorded checksum covers added/removed
/// outputs, include-pattern edits, dependency changes, etc.), or — for plans
/// deployed before provenance was recorded, which have no checksum — when
/// the declared outputs no longer match the parts registered for the plan.
async fn plan_is_outdated(
    db: &InstalledDb,
    manifest: &PlanManifest,
    plan: &PlanRecord,
) -> Result<bool> {
    let triple_changed = plan.epoch != manifest.metadata.epoch as i64
        || plan.release != manifest.metadata.release as i64
        || plan.version != manifest.metadata.version.as_deref().unwrap_or("");
    if triple_changed {
        return Ok(true);
    }

    if let (Some(deployed), Some(current)) = (&plan.plan_checksum, &manifest.plan_checksum) {
        return Ok(deployed != current);
    }

    // Legacy deployments carry no plan checksum: fall back to comparing the
    // manifest's declared outputs against the registered parts.
    let expected: HashSet<String> = manifest_part_names(manifest).into_iter().collect();
    let installed: HashSet<String> = db
        .get_parts_by_plan(&plan.name)
        .await?
        .into_iter()
        .map(|p| p.name)
        .collect();
    Ok(expected != installed)
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

        if plan_is_outdated(&db, &manifest, &plan).await? {
            outdated.push(target.clone());
        }
    }

    Ok(outdated)
}

/// Scan every installed plan, compare the deployed record with the current
/// plan manifest, and return the names of plans that are outdated.
async fn find_outdated_plans(config: &GlobalConfig, db_path: &Path) -> Result<Vec<String>> {
    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open database: {}", e)))?;

    let plans = db
        .list_plans()
        .await
        .map_err(|e| WrightError::DatabaseError(format!("list plans: {}", e)))?;

    let plan_dirs = plan_search_dirs(config);
    let index = PlanIndex::discover(&plan_dirs)?;

    let mut outdated = Vec::new();
    for plan in plans {
        let Some(plan_path) = index.path_for(&plan.name) else {
            continue; // no local plan for this installed plan
        };

        let manifest = match PlanManifest::from_file(plan_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if plan_is_outdated(&db, &manifest, &plan).await? {
            outdated.push(plan.name.clone());
        }
    }
    Ok(outdated)
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
            describe_rdep(
                "aegis",
                Some(&RebuildReason::LinkDependency),
                Some(&trigger)
            ),
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

    /// Same manifest version as deployed, but the plan gained an output and
    /// the deployment predates provenance recording (no plan checksum): the
    /// output-set fallback must flag the plan as outdated.
    #[tokio::test]
    async fn added_output_without_version_bump_is_detected_as_outdated() {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        let plan_dir = plans_dir.join("split-plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(
            plan_dir.join("plan.toml"),
            r#"
name = "split-plan"
version = "1.0.0"
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
        // Only the runtime output is deployed; split-devel was added later.
        db.insert_part(NewPart {
            name: "split-runtime",
            plan_id,
            ..Default::default()
        })
        .await
        .unwrap();
        drop(db);

        let mut config = GlobalConfig::default();
        config.general.plans_dir = plans_dir;

        let outdated = filter_outdated_targets(&["split-plan".to_string()], &config, &db_path)
            .await
            .unwrap();
        assert_eq!(outdated, ["split-plan"]);
    }

    /// Same version triple, but the recorded plan checksum no longer matches
    /// the manifest on disk (e.g. an output was added): outdated.
    #[tokio::test]
    async fn changed_plan_checksum_is_detected_as_outdated() {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        let plan_dir = plans_dir.join("solo");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(
            plan_dir.join("plan.toml"),
            r#"
name = "solo"
version = "1.0.0"
release = 1
description = "solo plan"
license = "MIT"
arch = "x86_64"
"#,
        )
        .unwrap();

        let db_path = temp.path().join("wright.db");
        let db = InstalledDb::open(&db_path).await.unwrap();
        let plan_id = db
            .insert_plan(NewPlan {
                name: "solo",
                version: "1.0.0",
                release: 1,
                epoch: 0,
                arch: "x86_64",
            })
            .await
            .unwrap();
        db.insert_part(NewPart {
            name: "solo",
            plan_id,
            ..Default::default()
        })
        .await
        .unwrap();
        db.set_plan_provenance(
            plan_id,
            wright_state::database::NewPlanProvenance {
                plan_checksum: Some(
                    "0000000000000000000000000000000000000000000000000000000000000000",
                ),
                source_checksums: &[],
                wright_version: "test",
                isolation: "none",
            },
        )
        .await
        .unwrap();
        drop(db);

        let mut config = GlobalConfig::default();
        config.general.plans_dir = plans_dir;

        let outdated = filter_outdated_targets(&["solo".to_string()], &config, &db_path)
            .await
            .unwrap();
        assert_eq!(outdated, ["solo"]);
    }

    /// Same version triple and a matching plan checksum: up to date.
    #[tokio::test]
    async fn matching_plan_checksum_is_up_to_date() {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        let plan_dir = plans_dir.join("solo");
        std::fs::create_dir_all(&plan_dir).unwrap();
        let plan_path = plan_dir.join("plan.toml");
        std::fs::write(
            &plan_path,
            r#"
name = "solo"
version = "1.0.0"
release = 1
description = "solo plan"
license = "MIT"
arch = "x86_64"
"#,
        )
        .unwrap();
        let checksum = wright_plan::manifest::PlanManifest::from_file(&plan_path)
            .unwrap()
            .plan_checksum
            .unwrap();

        let db_path = temp.path().join("wright.db");
        let db = InstalledDb::open(&db_path).await.unwrap();
        let plan_id = db
            .insert_plan(NewPlan {
                name: "solo",
                version: "1.0.0",
                release: 1,
                epoch: 0,
                arch: "x86_64",
            })
            .await
            .unwrap();
        db.insert_part(NewPart {
            name: "solo",
            plan_id,
            ..Default::default()
        })
        .await
        .unwrap();
        db.set_plan_provenance(
            plan_id,
            wright_state::database::NewPlanProvenance {
                plan_checksum: Some(&checksum),
                source_checksums: &[],
                wright_version: "test",
                isolation: "none",
            },
        )
        .await
        .unwrap();
        drop(db);

        let mut config = GlobalConfig::default();
        config.general.plans_dir = plans_dir;

        let outdated = filter_outdated_targets(&["solo".to_string()], &config, &db_path)
            .await
            .unwrap();
        assert!(outdated.is_empty());
    }
}

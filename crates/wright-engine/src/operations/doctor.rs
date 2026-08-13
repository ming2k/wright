use std::collections::HashSet;
use std::path::Path;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_model::version::{self, DepRef};
use wright_part::archive::read_archive_meta;
use wright_state::database::InstalledDb;

/// Run comprehensive system health checks.
///
/// Delegates to `health::run_standard_checks` (integrity + files + deps + ELF)
/// and additionally verifies the dependency closure of archives in parts_dir.
pub async fn execute_doctor(
    db: &InstalledDb,
    root_dir: &Path,
    config: &GlobalConfig,
) -> Result<()> {
    let t0 = crate::util::timing::WorkflowTiming::new();
    crate::cli_action!("Checking", "system health");

    let mut total_issues = super::health::run_standard_checks(
        db, root_dir, None,  // only_parts
        true,  // deep
        false, // integrity_only
        true,  // check_files
    )
    .await?
    .total_issues;

    let closure_issues = check_parts_dir_closure(config).await?;
    total_issues += closure_issues;

    // Advisory only (ADR-0023): drift means "rebuild to converge", not a
    // health failure, so it is reported without contributing to the issue
    // count that fails doctor.
    check_plan_drift(db, config).await?;

    if total_issues == 0 {
        crate::cli_action!(
            "Finished",
            "doctor in {}: clean",
            crate::util::timing::format_duration(t0.elapsed())
        );
        Ok(())
    } else {
        Err(WrightError::DependencyError(format!(
            "doctor found {} issue(s)",
            total_issues
        )))
    }
}

/// Scan parts_dir and verify that every archive's runtime_deps can be
/// resolved to a provider archive in the same directory.
async fn check_parts_dir_closure(config: &GlobalConfig) -> Result<usize> {
    let parts_dir = &config.general.parts_dir;
    if !parts_dir.exists() {
        return Ok(0);
    }

    // Recurse: current archives live in per-plan subdirectories, older
    // ones flat at the top level. Identity comes from .PARTINFO either way.
    let mut metas = Vec::new();
    for entry in walkdir::WalkDir::new(parts_dir).into_iter().flatten() {
        let path = entry.path();
        let is_archive = path
            .file_name()
            .and_then(|f| f.to_str())
            .map(|n| n.ends_with(".wright.tar.zst"))
            .unwrap_or(false);
        if !is_archive {
            continue;
        }
        match read_archive_meta(path) {
            Ok(meta) => metas.push(meta),
            Err(e) => {
                crate::cli_warn!("skipping unreadable archive {}: {}", path.display(), e);
            }
        }
    }
    if metas.is_empty() {
        return Ok(0);
    }

    crate::cli_action!("Checking", "dependency closure ({} archives)", metas.len());

    // Providers are (plan, output) pairs; several plans may ship same-named
    // outputs, so a qualified dep must match its plan exactly while a bare
    // dep matches any plan's output (or plan) of that name.
    let providers: HashSet<(String, String)> = metas
        .iter()
        .map(|meta| (meta.partinfo.plan.name.clone(), meta.partinfo.name.clone()))
        .collect();

    let mut missing: Vec<String> = Vec::new();

    for meta in &metas {
        for dep in &meta.partinfo.runtime_deps {
            let dep = dep.trim();
            if dep.is_empty() {
                continue;
            }
            let Some(target) = resolve_dep_target(dep) else {
                continue;
            };
            let satisfied = match &target {
                DepTarget::Name(name) => providers
                    .iter()
                    .any(|(plan, output)| output == name || plan == name),
                DepTarget::PlanOutput { plan, output } => {
                    providers.iter().any(|(p, o)| p == plan && o == output)
                }
            };
            if !satisfied {
                missing.push(format!(
                    "{} needs {} (no provider in parts_dir)",
                    meta.partinfo.name, dep
                ));
            }
        }
    }

    if !missing.is_empty() {
        crate::cli_warn!("{} missing runtime dependencies", missing.len());
        for line in &missing {
            // Indent each finding under the warning line; one bullet per
            // missing dep keeps the output scannable.
            crate::util::progress::term_println(&format!("             - {}", line));
        }
    }

    Ok(missing.len())
}

/// Compare each registered plan's recorded provenance checksum against the
/// current plan source on disk. A mismatch means the plan changed since its
/// parts were sealed — the installed state no longer reflects plan source.
async fn check_plan_drift(db: &InstalledDb, config: &GlobalConfig) -> Result<usize> {
    let plans = db.list_plans().await?;
    if plans.iter().all(|p| p.plan_checksum.is_none()) {
        return Ok(0);
    }

    let plan_dirs = crate::resolve::plan_search_dirs(config);
    let index = match wright_plan::discovery::PlanIndex::discover(&plan_dirs) {
        Ok(index) => index,
        Err(e) => {
            crate::cli_warn!("skipping plan drift check: {}", e);
            return Ok(0);
        }
    };

    let mut drifted: Vec<(String, Option<String>)> = Vec::new();
    for plan in &plans {
        let Some(ref recorded) = plan.plan_checksum else {
            continue;
        };
        let Some(path) = index.path_for(&plan.name) else {
            continue;
        };
        match crate::util::checksum::sha256_file(path) {
            Ok(current) if &current != recorded => {
                let detail = format!(
                    "{} (installed from {}…, source now {}…)",
                    plan.name,
                    &recorded[..12.min(recorded.len())],
                    &current[..12]
                );
                let diff = plan_drift_diff(db, recorded, path).await;
                drifted.push((detail, diff));
            }
            Ok(_) => {}
            Err(e) => crate::cli_warn!("cannot checksum {}: {}", path.display(), e),
        }
    }

    if !drifted.is_empty() {
        crate::cli_warn!(
            "{} plan(s) changed since their parts were installed (advisory; rebuild to converge)",
            drifted.len()
        );
        for (line, diff) in &drifted {
            crate::util::progress::term_println(&format!("             - {}", line));
            if let Some(diff) = diff {
                for diff_line in diff.lines() {
                    crate::util::progress::term_println(&format!("               {}", diff_line));
                }
            }
        }
    }

    Ok(drifted.len())
}

/// Unified diff between the plan-source snapshot recorded at seal time and
/// the current source on disk (ADR-0033). `None` when no snapshot exists
/// (parts sealed before snapshots) or the current source cannot be read —
/// drift reporting is advisory, so diff problems degrade to the checksum
/// line alone.
async fn plan_drift_diff(db: &InstalledDb, recorded: &str, current_path: &Path) -> Option<String> {
    let snapshot = db.get_plan_snapshot(recorded).await.ok()??;
    let current = std::fs::read_to_string(current_path).ok()?;
    let diff = similar::TextDiff::from_lines(&snapshot, &current)
        .unified_diff()
        .header(
            &format!(
                "installed snapshot {}…",
                &recorded[..12.min(recorded.len())]
            ),
            "current plan source",
        )
        .to_string();
    if diff.trim().is_empty() {
        None
    } else {
        Some(diff)
    }
}

/// A runtime-dep target for the closure check: either a bare name that any
/// plan's output (or plan) may satisfy, or an absolute `plan:output` pair
/// that only that plan's output satisfies.
enum DepTarget {
    Name(String),
    PlanOutput { plan: String, output: String },
}

fn resolve_dep_target(dep: &str) -> Option<DepTarget> {
    let (dep_ref, _) = version::parse_dependency(dep).ok()?;
    match version::parse_dep_ref(&dep_ref) {
        DepRef::Specific(plan, output) => Some(DepTarget::PlanOutput { plan, output }),
        DepRef::Wildcard(name) => Some(DepTarget::Name(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::{NewPlan, NewPlanProvenance, RegisterPlan};

    async fn register_plan_with_snapshot(db: &InstalledDb, checksum: &str, source: &str) {
        db.ensure_plan_registered(RegisterPlan {
            plan: NewPlan {
                name: "demo",
                version: "1.0.0",
                release: 1,
                epoch: 0,
                arch: "x86_64",
            },
            provenance: Some(NewPlanProvenance {
                plan_checksum: Some(checksum),
                source_checksums: &[],
                wright_version: "test",
                isolation: "none",
            }),
        })
        .await
        .unwrap();
        db.insert_plan_snapshot(checksum, source).await.unwrap();
    }

    #[tokio::test]
    async fn drift_diff_shows_snapshot_against_current_source() {
        let db = InstalledDb::open_in_memory().await.unwrap();
        register_plan_with_snapshot(&db, "deadbeefcafe", "release = 1\n").await;

        let dir = tempfile::tempdir().unwrap();
        let plan_path = dir.path().join("plan.toml");
        std::fs::write(&plan_path, "release = 2\n").unwrap();

        let diff = plan_drift_diff(&db, "deadbeefcafe", &plan_path)
            .await
            .expect("snapshot exists, diff expected");
        assert!(diff.contains("-release = 1"), "diff was: {}", diff);
        assert!(diff.contains("+release = 2"), "diff was: {}", diff);
        assert!(
            diff.contains("installed snapshot deadbeefcafe…"),
            "diff was: {}",
            diff
        );
    }

    #[tokio::test]
    async fn drift_diff_degrades_without_snapshot() {
        let db = InstalledDb::open_in_memory().await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let plan_path = dir.path().join("plan.toml");
        std::fs::write(&plan_path, "release = 2\n").unwrap();

        assert!(
            plan_drift_diff(&db, "nosuchchecksum", &plan_path)
                .await
                .is_none()
        );
    }
}

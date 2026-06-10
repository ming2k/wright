use std::path::Path;

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::part::archive::read_archive_meta;
use crate::part::soname::SonameIndex;
use crate::part::version;

/// Run comprehensive system health checks.
///
/// Delegates to `health::run_standard_checks` (integrity + files + deps + ELF)
/// and additionally verifies the dependency closure of archives in parts_dir.
pub async fn execute_doctor(
    db: &InstalledDb,
    root_dir: &Path,
    config: &GlobalConfig,
) -> Result<()> {
    let t0 = std::time::Instant::now();
    crate::cli_action!("Checking", "system health");

    let mut total_issues = super::health::run_standard_checks(
        db, root_dir, None,  // only_part
        true,  // deep
        false, // integrity_only
        true,  // check_files
    )
    .await?;

    let closure_issues = check_parts_dir_closure(config).await?;
    total_issues += closure_issues;

    // Advisory only (ADR-0023): drift means "rebuild to converge", not a
    // health failure, so it is reported without contributing to the issue
    // count that fails doctor.
    check_plan_drift(db, config).await?;

    let elapsed = t0.elapsed().as_secs_f64();
    if total_issues == 0 {
        crate::cli_action!(
            "Finished",
            "doctor in {}: clean",
            crate::foundry::logging::format_duration(elapsed)
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

    let mut archive_count = 0usize;
    for entry in std::fs::read_dir(parts_dir)
        .map_err(|e| WrightError::PartError(format!("read {}: {}", parts_dir.display(), e)))?
        .flatten()
    {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|f| f.to_str())
            .map(|n| n.ends_with(".wright.tar.zst"))
            .unwrap_or(false)
        {
            archive_count += 1;
        }
    }
    if archive_count == 0 {
        return Ok(0);
    }

    crate::cli_action!(
        "Checking",
        "dependency closure ({} archives)",
        archive_count
    );

    let index = SonameIndex::scan_parts_dir(parts_dir).unwrap_or_else(|e| {
        crate::cli_warn!("failed to build SONAME index: {}", e);
        SonameIndex::default()
    });

    let mut missing: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(parts_dir)
        .map_err(|e| WrightError::PartError(format!("read {}: {}", parts_dir.display(), e)))?
        .flatten()
    {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|f| f.to_str())
            .map(|n| n.ends_with(".wright.tar.zst"))
            .unwrap_or(false)
        {
            continue;
        }

        let meta = match read_archive_meta(&path) {
            Ok(m) => m,
            Err(e) => {
                crate::cli_warn!("skipping unreadable archive {}: {}", path.display(), e);
                continue;
            }
        };

        for dep in &meta.partinfo.runtime_deps {
            let dep = dep.trim();
            if dep.is_empty() {
                continue;
            }
            let targets = resolve_dep_targets(dep, &index);
            if targets.is_empty() {
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
            let _ = crate::util::progress::MULTI.println(format!("             - {}", line));
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
    let index = match crate::plan::discovery::PlanIndex::discover(&plan_dirs) {
        Ok(index) => index,
        Err(e) => {
            crate::cli_warn!("skipping plan drift check: {}", e);
            return Ok(0);
        }
    };

    let mut drifted: Vec<String> = Vec::new();
    for plan in &plans {
        let Some(ref recorded) = plan.plan_checksum else {
            continue;
        };
        let Some(path) = index.path_for(&plan.name) else {
            continue;
        };
        match crate::util::checksum::sha256_file(path) {
            Ok(current) if &current != recorded => {
                drifted.push(format!(
                    "{} (installed from {}…, source now {}…)",
                    plan.name,
                    &recorded[..12.min(recorded.len())],
                    &current[..12]
                ));
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
        for line in &drifted {
            let _ = crate::util::progress::MULTI.println(format!("             - {}", line));
        }
    }

    Ok(drifted.len())
}

fn resolve_dep_targets(dep: &str, index: &SonameIndex) -> Vec<String> {
    let mut targets = Vec::new();
    if dep.is_empty() {
        return targets;
    }
    let (dep_ref, _) = match version::parse_dependency(dep) {
        Ok(parsed) => parsed,
        Err(_) => return targets,
    };
    let (plan, output) = version::parse_dep_ref(&dep_ref).to_plan_output();
    if !output.is_empty() {
        targets.push(output);
    } else if let Some(outs) = index.outputs_of(&plan) {
        for o in outs {
            targets.push(o.clone());
        }
    } else {
        targets.push(plan);
    }
    targets
}

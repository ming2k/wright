use std::path::Path;

use owo_colors::OwoColorize;

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::operations::check;
use crate::part::archive::read_archive_meta;
use crate::part::lint::SonameIndex;
use crate::part::version;

/// Run comprehensive system health checks.
///
/// Performs all checks from `check --deep` and additionally verifies the
/// dependency closure of archives in parts_dir. Intended to be run manually
/// after batch installations.
pub async fn execute_doctor(
    db: &InstalledDb,
    root_dir: &Path,
    config: &GlobalConfig,
) -> Result<()> {
    let mut total_issues = 0usize;

    println!("Running system health checks...\n");

    // 1. Integrity checks (database, file conflicts, shadows)
    total_issues += check::integrity_check(db).await?;
    println!();

    // 2. Registry-level dependency checks
    let registry_findings = check::registry_check(db, None).await?;
    check::print_registry_findings(&registry_findings);
    total_issues += registry_findings.len();

    // 3. ELF-level dependency verification (like `check --deep`)
    let elf_findings = check::elf_check(db, root_dir, None).await?;
    check::print_elf_findings(&elf_findings);
    total_issues += elf_findings.missing.len() + elf_findings.unmapped.len();

    // 4. Global parts_dir dependency closure check
    let closure_issues = check_parts_dir_closure(config).await?;
    total_issues += closure_issues;

    if total_issues == 0 {
        println!("{}: system reports clean", "doctor".green());
        Ok(())
    } else {
        Err(WrightError::DependencyError(format!(
            "{} issue(s) reported by `wright doctor`",
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

    // Count archives first — skip the check entirely when the directory
    // is empty so we do not report false-positives.
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

    println!("Checking parts_dir dependency closure...");

    // Build a global SONAME index from *all* archives in parts_dir.
    let index = SonameIndex::scan_parts_dir(parts_dir).unwrap_or_else(|e| {
        tracing::warn!("doctor: failed to build SONAME index: {}", e);
        SonameIndex::default()
    });

    let mut issues = 0usize;

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
                tracing::warn!(
                    "doctor: skipping unreadable archive {}: {}",
                    path.display(),
                    e
                );
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
                println!(
                    "  {}: '{}' declares '{}' but no archive provides it",
                    "missing".red(),
                    meta.partinfo.name,
                    dep
                );
                issues += 1;
            }
        }
    }

    if issues == 0 {
        println!("  {}: all runtime_deps resolve in parts_dir", "OK".green());
    }

    Ok(issues)
}

/// Resolve a dependency string to the set of output names it could route to,
/// using the global SONAME index.
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
        // Unknown plan — fall back to treating the token as an output name,
        // consistent with the lint logic in `targets_for_dep`.
        targets.push(plan);
    }
    targets
}

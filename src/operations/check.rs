use std::collections::BTreeMap;
use std::path::Path;

use owo_colors::OwoColorize;

use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::part::elf;
use crate::query;

/// Implementation of `wright check`.
///
/// Runs system health checks covering:
/// - Database integrity
/// - File ownership conflicts and shadowed files
/// - Runtime dependency resolution (registry level)
/// - With `--deep`: ELF DT_NEEDED verification
///
/// Exit semantics: returns `WrightError::DependencyError` when any
/// problem is found, so the CLI dispatch layer maps to a non-zero exit.
pub async fn execute_check(
    db: &InstalledDb,
    root_dir: &Path,
    only_part: Option<&str>,
    deep: bool,
    integrity_only: bool,
) -> Result<()> {
    let mut total_issues = 0usize;

    // --- Integrity checks (from doctor) ---
    let integrity_issues = integrity_check(db).await?;
    total_issues += integrity_issues;

    if integrity_only {
        if total_issues == 0 {
            println!("{}: system integrity reports clean", "check".green());
        }
        return Ok(());
    }

    // --- Dependency checks (from original check) ---
    let registry_findings = registry_check(db, only_part).await?;
    let elf_findings = if deep {
        elf_check(db, root_dir, only_part).await?
    } else {
        DeepReport::default()
    };

    print_registry_findings(&registry_findings);
    if deep {
        print_elf_findings(&elf_findings);
    }

    total_issues +=
        registry_findings.len() + elf_findings.missing.len() + elf_findings.unmapped.len();

    if total_issues == 0 {
        let scope = scope_label(only_part);
        let mode = if deep {
            " (integrity + deps + ELF)"
        } else {
            " (integrity + deps)"
        };
        println!("{}: {} reports clean{}", "check".green(), scope, mode);
        return Ok(());
    }

    Err(WrightError::DependencyError(format!(
        "{} issue(s) reported by `wright check{}`",
        total_issues,
        if deep { " --deep" } else { "" }
    )))
}

pub(super) async fn integrity_check(db: &InstalledDb) -> Result<usize> {
    let mut issues = 0usize;

    // 1. Database integrity
    print!("Checking database integrity... ");
    match db.integrity_check().await {
        Ok(issues_list) if issues_list.is_empty() => println!("{}", "OK".green()),
        Ok(issues_list) => {
            println!("{}", "FAILED".red());
            for issue in issues_list {
                println!("  - {}", issue);
                issues += 1;
            }
        }
        Err(e) => {
            println!("{} ({})", "ERROR".red(), e);
            issues += 1;
        }
    }

    // 2. Shadowed file conflicts
    print!("Checking for file shadowing conflicts... ");
    match db.get_shadowed_conflicts().await {
        Ok(conflicts) if conflicts.is_empty() => println!("{}", "OK".green()),
        Ok(conflicts) => {
            println!("{}", "WARNING".yellow());
            for conflict in conflicts {
                println!("  - {}", conflict);
                issues += 1;
            }
        }
        Err(e) => {
            println!("{} ({})", "ERROR".red(), e);
            issues += 1;
        }
    }

    Ok(issues)
}

fn scope_label(only_part: Option<&str>) -> String {
    match only_part {
        Some(p) => format!("part '{}'", p),
        None => "registry".to_string(),
    }
}

pub(super) async fn registry_check(
    db: &InstalledDb,
    only_part: Option<&str>,
) -> Result<Vec<query::BrokenDep>> {
    let mut broken = query::check_dependencies_structured(db).await?;
    if let Some(filter) = only_part {
        broken.retain(|b| b.part == filter);
    }
    Ok(broken)
}

pub(super) fn print_registry_findings(broken: &[query::BrokenDep]) {
    if broken.is_empty() {
        return;
    }
    println!(
        "{}: {} unsatisfied registry edge(s)",
        "advisory".yellow(),
        broken.len()
    );
    for b in broken {
        let vc = b
            .version_constraint
            .as_deref()
            .map(|c| format!(" ({})", c))
            .unwrap_or_default();
        println!("  {} → {}{}", b.part.bold(), b.required_name, vc);
    }
    println!();
}

#[derive(Default)]
pub(super) struct DeepReport {
    /// SONAMEs an ELF binary needs but no installed file provides.
    pub(super) missing: Vec<DeepMissing>,
    /// SONAMEs whose owner is the installed file table but whose owner
    /// is the part itself (purely informational; not reported).
    /// Kept here for clarity even though we filter it out.
    #[allow(dead_code)]
    pub(super) self_links: usize,
    /// Internal accounting; not reported.
    pub(super) unmapped: Vec<DeepMissing>,
}

pub(super) struct DeepMissing {
    part: String,
    binary: String,
    soname: String,
}

pub(super) async fn elf_check(
    db: &InstalledDb,
    root_dir: &Path,
    only_part: Option<&str>,
) -> Result<DeepReport> {
    let mut report = DeepReport::default();

    let parts: Vec<(i64, String)> = match only_part {
        Some(name) => match db.get_part(name).await? {
            Some(p) => vec![(p.id, p.name)],
            None => {
                return Err(WrightError::PartNotFound(name.to_string()));
            }
        },
        None => db
            .list_parts()
            .await?
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect(),
    };

    // SONAME lookup: resolve a needed soname to the part that owns the
    // matching file path. Built lazily on the first miss to avoid scanning
    // the entire files table for systems with no broken edges.
    let mut soname_owner_cache: BTreeMap<String, Option<String>> = BTreeMap::new();

    for (part_id, part_name) in &parts {
        let files = db.get_files(*part_id).await?;
        for f in &files {
            if f.file_type != crate::database::FileType::File {
                continue;
            }
            let abs = root_dir.join(f.path.trim_start_matches('/'));
            if !abs.exists() {
                continue;
            }
            let needed = match elf::read_dt_needed(&abs) {
                Ok(Some(libs)) => libs,
                Ok(None) | Err(_) => continue,
            };
            for soname in needed {
                let owner = match soname_owner_cache.get(&soname) {
                    Some(cached) => cached.clone(),
                    None => {
                        let owner = resolve_soname_owner(db, &soname).await?;
                        soname_owner_cache.insert(soname.clone(), owner.clone());
                        owner
                    }
                };
                match owner {
                    Some(owner_name) => {
                        if owner_name == *part_name {
                            // self-link; ignore
                        }
                        // else: satisfied, nothing to report
                    }
                    None => {
                        // The advisory model still reports this so the
                        // user can investigate; host-provided libs should
                        // be `wright assume`'d to suppress.
                        report.missing.push(DeepMissing {
                            part: part_name.clone(),
                            binary: f.path.clone(),
                            soname,
                        });
                    }
                }
            }
        }
    }

    Ok(report)
}

/// Resolve a SONAME to the deployed part that owns a file with that
/// basename. Walks `files.path` looking for a path whose final segment
/// equals the SONAME. The first match wins.
async fn resolve_soname_owner(db: &InstalledDb, soname: &str) -> Result<Option<String>> {
    // Walk all parts; for each, walk its files; basename-match.
    let parts = db.list_parts().await?;
    for p in parts {
        let files = db.get_files(p.id).await?;
        for f in files {
            if let Some(base) = f.path.rsplit('/').next() {
                if base == soname {
                    return Ok(Some(p.name));
                }
            }
        }
    }
    Ok(None)
}

pub(super) fn print_elf_findings(report: &DeepReport) {
    if report.missing.is_empty() {
        return;
    }
    println!(
        "{}: {} ELF binary load(s) cannot be resolved",
        "advisory".yellow(),
        report.missing.len()
    );
    for m in &report.missing {
        println!(
            "  {} ({}) needs {} — no deployed part owns this SONAME",
            m.part.bold(),
            m.binary,
            m.soname.red()
        );
    }
    println!();
    println!(
        "These binaries will fail to start with `error while loading shared \
         libraries`. Install the providing part or `wright assume` it as \
         externally provided."
    );
}

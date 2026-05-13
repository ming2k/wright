use std::collections::BTreeMap;
use std::path::Path;

use owo_colors::OwoColorize;

use crate::database::{FileType, InstalledDb, InstalledPart, Origin};
use crate::error::{Result, WrightError};
use crate::part::elf;
use crate::query;

/// Run the standard suite of system health checks and return the total issue
/// count. Callers format their own final messages (e.g. `check` vs `doctor`
/// branding).
pub(super) async fn run_standard_checks(
    db: &InstalledDb,
    root_dir: &Path,
    only_part: Option<&str>,
    deep: bool,
    integrity_only: bool,
    check_files: bool,
) -> Result<usize> {
    let mut total_issues = 0usize;

    let integrity_issues = integrity_check(db).await?;
    total_issues += integrity_issues;

    if integrity_only {
        return Ok(total_issues);
    }

    if check_files {
        println!();
        let files_issues = files_check(db, root_dir, only_part).await?;
        total_issues += files_issues;
    }

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

    Ok(total_issues)
}

// ── integrity ───────────────────────────────────────────────────────────

async fn integrity_check(db: &InstalledDb) -> Result<usize> {
    let mut issues = 0usize;

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

// ── registry deps ───────────────────────────────────────────────────────

async fn registry_check(
    db: &InstalledDb,
    only_part: Option<&str>,
) -> Result<Vec<query::BrokenDep>> {
    let mut broken = query::check_dependencies_structured(db).await?;
    if let Some(filter) = only_part {
        broken.retain(|b| b.part == filter);
    }
    Ok(broken)
}

fn print_registry_findings(broken: &[query::BrokenDep]) {
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

// ── ELF deep check ──────────────────────────────────────────────────────

#[derive(Default)]
struct DeepReport {
    missing: Vec<DeepMissing>,
    #[allow(dead_code)]
    self_links: usize,
    unmapped: Vec<DeepMissing>,
}

struct DeepMissing {
    part: String,
    binary: String,
    soname: String,
}

async fn elf_check(
    db: &InstalledDb,
    root_dir: &Path,
    only_part: Option<&str>,
) -> Result<DeepReport> {
    let mut report = DeepReport::default();

    let parts: Vec<(i64, String)> = match only_part {
        Some(name) => match db.get_part(name).await? {
            Some(p) => vec![(p.id, p.name)],
            None => return Err(WrightError::PartNotFound(name.to_string())),
        },
        None => db
            .list_parts()
            .await?
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect(),
    };

    let mut soname_owner_cache: BTreeMap<String, Option<String>> = BTreeMap::new();

    for (part_id, part_name) in &parts {
        let files = db.get_files(*part_id).await?;
        for f in &files {
            if f.file_type != FileType::File {
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
                    Some(owner_name) if owner_name != *part_name => {}
                    Some(_) => {}
                    None => report.missing.push(DeepMissing {
                        part: part_name.clone(),
                        binary: f.path.clone(),
                        soname,
                    }),
                }
            }
        }
    }

    Ok(report)
}

async fn resolve_soname_owner(db: &InstalledDb, soname: &str) -> Result<Option<String>> {
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

fn print_elf_findings(report: &DeepReport) {
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

// ── file existence ──────────────────────────────────────────────────────

struct FilesReport {
    missing: Vec<PartMissing>,
}

struct PartMissing {
    part_name: String,
    paths: Vec<String>,
}

async fn files_check(db: &InstalledDb, root_dir: &Path, only_part: Option<&str>) -> Result<usize> {
    print!("Checking deployed file existence... ");

    let parts: Vec<InstalledPart> = match only_part {
        Some(name) => match db.get_part(name).await? {
            Some(p) => vec![p],
            None => return Err(WrightError::PartNotFound(name.to_string())),
        },
        None => db
            .list_parts()
            .await?
            .into_iter()
            .map(|p| InstalledPart {
                id: p.id,
                name: p.name,
                plan_id: p.plan_id,
                installed_at: p.installed_at,
                part_hash: p.part_hash,
                deploy_scripts: p.deploy_scripts,
                origin: p.origin,
            })
            .collect(),
    };

    let mut report = FilesReport {
        missing: Vec::new(),
    };
    let mut total_missing = 0usize;
    let mut total_checked = 0usize;

    for part in &parts {
        if part.origin == Origin::External {
            continue;
        }

        let files = db.get_files(part.id).await?;
        total_checked += files.len();

        let mut missing_paths: Vec<String> = Vec::new();

        for f in &files {
            let abs = root_dir.join(f.path.trim_start_matches('/'));
            match f.file_type {
                FileType::File => {
                    if !abs.is_file() {
                        let extra = if abs.exists() {
                            " (wrong type)".to_string()
                        } else {
                            String::new()
                        };
                        missing_paths.push(format!("{}{}", abs.display(), extra));
                    }
                }
                FileType::Symlink => {
                    if !abs.is_symlink() {
                        let extra = if abs.exists() {
                            " (expected symlink)".to_string()
                        } else {
                            String::new()
                        };
                        missing_paths.push(format!("{}{}", abs.display(), extra));
                    }
                }
                FileType::Directory => {
                    if !abs.is_dir() {
                        let extra = if abs.exists() {
                            " (expected directory)".to_string()
                        } else {
                            String::new()
                        };
                        missing_paths.push(format!("{}{}", abs.display(), extra));
                    }
                }
            }
        }

        if !missing_paths.is_empty() {
            total_missing += missing_paths.len();
            report.missing.push(PartMissing {
                part_name: part.name.clone(),
                paths: missing_paths,
            });
        }
    }

    if total_missing == 0 {
        println!("{} ({} file(s) verified)", "OK".green(), total_checked);
        return Ok(0);
    }

    println!("{}", "FAILED".red());
    print_files_findings(&report);

    Ok(total_missing)
}

fn print_files_findings(report: &FilesReport) {
    let part_count = report.missing.len();
    let total_files: usize = report.missing.iter().map(|p| p.paths.len()).sum();

    println!(
        "  {}: {} missing file(s) across {} part(s)",
        "missing".red(),
        total_files,
        part_count
    );

    for pm in &report.missing {
        let count = pm.paths.len();
        if count <= 5 {
            for path in &pm.paths {
                println!("    {}: {}", pm.part_name, path);
            }
        } else {
            for path in pm.paths.iter().take(3) {
                println!("    {}: {}", pm.part_name, path);
            }
            println!("    {}: ... and {} more", pm.part_name, count - 3);
        }
    }

    println!();
    println!("To repair, reinstall the affected part(s):\n  wright install --force <part>");
}

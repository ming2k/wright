use std::collections::BTreeMap;
use std::path::Path;

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

    total_issues += integrity_check(db).await?;
    if integrity_only {
        return Ok(total_issues);
    }

    if check_files {
        total_issues += files_check(db, root_dir, only_part).await?;
    }

    let registry_findings = registry_check(db, only_part).await?;
    let elf_findings = if deep {
        elf_check(db, root_dir, only_part).await?
    } else {
        DeepReport::default()
    };

    report_registry_findings(&registry_findings);
    if deep {
        report_elf_findings(&elf_findings);
    }

    total_issues +=
        registry_findings.len() + elf_findings.missing.len() + elf_findings.unmapped.len();

    Ok(total_issues)
}

/// Emit a list of bullet findings indented under a verb line.
fn emit_bullets<I, S>(lines: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for line in lines {
        // 13 spaces lines up with the 12-col verb column + 1 pivot space.
        let _ = crate::util::progress::MULTI.println(format!("             - {}", line.as_ref()));
    }
}

// ── integrity ───────────────────────────────────────────────────────────

async fn integrity_check(db: &InstalledDb) -> Result<usize> {
    let mut issues = 0usize;

    crate::cli_action!("Checking", "database integrity");
    match db.integrity_check().await {
        Ok(list) if list.is_empty() => {}
        Ok(list) => {
            crate::cli_warn!("{} database integrity issue(s)", list.len());
            issues += list.len();
            emit_bullets(&list);
        }
        Err(e) => {
            crate::cli_error!("integrity check failed: {}", e);
            issues += 1;
        }
    }

    crate::cli_action!("Checking", "file shadowing conflicts");
    match db.get_shadowed_conflicts().await {
        Ok(list) if list.is_empty() => {}
        Ok(list) => {
            crate::cli_warn!("{} shadowed file conflict(s)", list.len());
            issues += list.len();
            emit_bullets(&list);
        }
        Err(e) => {
            crate::cli_error!("shadow check failed: {}", e);
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
    crate::cli_action!("Checking", "registry dependencies");
    let mut broken = query::check_dependencies_structured(db).await?;
    if let Some(filter) = only_part {
        broken.retain(|b| b.part == filter);
    }
    Ok(broken)
}

fn report_registry_findings(broken: &[query::BrokenDep]) {
    if broken.is_empty() {
        return;
    }
    crate::cli_warn!("{} unsatisfied registry edge(s)", broken.len());
    let lines: Vec<String> = broken
        .iter()
        .map(|b| {
            let vc = b
                .version_constraint
                .as_deref()
                .map(|c| format!(" ({})", c))
                .unwrap_or_default();
            format!("{} -> {}{}", b.part, b.required_name, vc)
        })
        .collect();
    emit_bullets(&lines);
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
    crate::cli_action!("Checking", "ELF dynamic loads");
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
            if let Some(base) = f.path.rsplit('/').next()
                && base == soname
            {
                return Ok(Some(p.name));
            }
        }
    }
    Ok(None)
}

fn report_elf_findings(report: &DeepReport) {
    if report.missing.is_empty() {
        return;
    }
    crate::cli_warn!(
        "{} ELF binary load(s) cannot be resolved",
        report.missing.len()
    );
    let lines: Vec<String> = report
        .missing
        .iter()
        .map(|m| {
            format!(
                "{} ({}) needs {} — no part provides this SONAME",
                m.part, m.binary, m.soname
            )
        })
        .collect();
    emit_bullets(&lines);
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
    crate::cli_action!("Checking", "deployed file existence");

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

    for part in &parts {
        if part.origin == Origin::External {
            continue;
        }

        let files = db.get_files(part.id).await?;
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
        return Ok(0);
    }

    let part_count = report.missing.len();
    crate::cli_warn!(
        "{} missing file(s) across {} part(s) — run `wright install --force <part>` to repair",
        total_missing,
        part_count
    );
    let mut lines: Vec<String> = Vec::new();
    for pm in &report.missing {
        let count = pm.paths.len();
        if count <= 5 {
            for path in &pm.paths {
                lines.push(format!("{}: {}", pm.part_name, path));
            }
        } else {
            for path in pm.paths.iter().take(3) {
                lines.push(format!("{}: {}", pm.part_name, path));
            }
            lines.push(format!("{}: ... and {} more", pm.part_name, count - 3));
        }
    }
    emit_bullets(&lines);

    Ok(total_missing)
}

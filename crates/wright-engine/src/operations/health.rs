use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{Result, WrightError};
use crate::query;
use wright_part::elf;
use wright_state::database::{FileType, InstalledDb, InstalledPart, Origin};

/// A single structured finding from the standard checks. Serialized as an
/// element of the `issues` array in `wright check --json` output; the `check`
/// tag names the check that produced the finding.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "check", rename_all = "kebab-case")]
pub(super) enum CheckIssue {
    /// Database referential-integrity problem reported by SQLite.
    DatabaseIntegrity { message: String },
    /// A file owned by more than one part (shadowing conflict).
    ShadowedFileConflict { message: String },
    /// A check itself failed to run; counts as one issue.
    CheckError { message: String },
    /// A deployed file is missing from disk or has the wrong type.
    MissingFile {
        part: String,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A part's recorded dependency edge is not satisfied.
    BrokenDependency {
        part: String,
        requires: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        constraint: Option<String>,
    },
    /// An ELF binary needs a SONAME that no deployed part provides.
    UnresolvedSoname {
        part: String,
        binary: String,
        soname: String,
    },
}

/// Outcome of the standard checks: the issue count that drives the exit
/// code, plus the structured findings behind `--json` output.
pub(super) struct CheckOutcome {
    pub total_issues: usize,
    pub issues: Vec<CheckIssue>,
}

/// Run the standard suite of system health checks. Callers format their own
/// final messages (e.g. `check` vs `doctor` branding).
pub(super) async fn run_standard_checks(
    db: &InstalledDb,
    root_dir: &Path,
    only_parts: Option<&[String]>,
    deep: bool,
    integrity_only: bool,
    check_files: bool,
) -> Result<CheckOutcome> {
    let mut total_issues = 0usize;
    let mut issues: Vec<CheckIssue> = Vec::new();

    let (integrity_issues, mut found) = integrity_check(db).await?;
    total_issues += integrity_issues;
    issues.append(&mut found);
    if integrity_only {
        return Ok(CheckOutcome {
            total_issues,
            issues,
        });
    }

    if check_files {
        let (file_issues, mut found) = files_check(db, root_dir, only_parts).await?;
        total_issues += file_issues;
        issues.append(&mut found);
    }

    let registry_findings = registry_check(db, only_parts).await?;
    let elf_findings = if deep {
        elf_check(db, root_dir, only_parts).await?
    } else {
        DeepReport::default()
    };

    report_registry_findings(&registry_findings);
    if deep {
        report_elf_findings(&elf_findings);
    }

    issues.extend(
        registry_findings
            .iter()
            .map(|b| CheckIssue::BrokenDependency {
                part: b.part.clone(),
                requires: b.required_name.clone(),
                constraint: b.version_constraint.clone(),
            }),
    );
    issues.extend(
        elf_findings
            .missing
            .iter()
            .map(|m| CheckIssue::UnresolvedSoname {
                part: m.part.clone(),
                binary: m.binary.clone(),
                soname: m.soname.clone(),
            }),
    );

    total_issues +=
        registry_findings.len() + elf_findings.missing.len() + elf_findings.unmapped.len();

    Ok(CheckOutcome {
        total_issues,
        issues,
    })
}

/// Emit a list of bullet findings indented under a verb line.
fn emit_bullets<I, S>(lines: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for line in lines {
        // 13 spaces lines up with the 12-col verb column + 1 pivot space.
        crate::util::progress::term_println(&format!("             - {}", line.as_ref()));
    }
}

// ── integrity ───────────────────────────────────────────────────────────

async fn integrity_check(db: &InstalledDb) -> Result<(usize, Vec<CheckIssue>)> {
    let mut issues = 0usize;
    let mut found = Vec::new();

    crate::cli_action!("Checking", "database integrity");
    match db.integrity_check().await {
        Ok(list) if list.is_empty() => {}
        Ok(list) => {
            crate::cli_warn!("{} database integrity issue(s)", list.len());
            issues += list.len();
            emit_bullets(&list);
            found.extend(
                list.into_iter()
                    .map(|message| CheckIssue::DatabaseIntegrity { message }),
            );
        }
        Err(e) => {
            crate::cli_error!("integrity check failed: {}", e);
            issues += 1;
            found.push(CheckIssue::CheckError {
                message: format!("integrity check failed: {}", e),
            });
        }
    }

    crate::cli_action!("Checking", "file shadowing conflicts");
    match db.get_shadowed_conflicts().await {
        Ok(list) if list.is_empty() => {}
        Ok(list) => {
            crate::cli_warn!("{} shadowed file conflict(s)", list.len());
            issues += list.len();
            emit_bullets(&list);
            found.extend(
                list.into_iter()
                    .map(|message| CheckIssue::ShadowedFileConflict { message }),
            );
        }
        Err(e) => {
            crate::cli_error!("shadow check failed: {}", e);
            issues += 1;
            found.push(CheckIssue::CheckError {
                message: format!("shadow check failed: {}", e),
            });
        }
    }

    Ok((issues, found))
}

// ── registry deps ───────────────────────────────────────────────────────

async fn registry_check(
    db: &InstalledDb,
    only_parts: Option<&[String]>,
) -> Result<Vec<query::BrokenDep>> {
    crate::cli_action!("Checking", "registry dependencies");
    let mut broken = query::check_dependencies_structured(db).await?;
    if let Some(names) = only_parts {
        broken.retain(|b| names.iter().any(|name| name == &b.part));
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
    only_parts: Option<&[String]>,
) -> Result<DeepReport> {
    crate::cli_action!("Checking", "ELF dynamic loads");
    let mut report = DeepReport::default();

    let parts: Vec<(i64, String)> = match only_parts {
        Some(names) => {
            let mut selected = Vec::with_capacity(names.len());
            for name in names {
                match db.get_part(name).await? {
                    Some(p) => selected.push((p.id, p.name)),
                    None => return Err(WrightError::PartNotFound(name.clone())),
                }
            }
            selected
        }
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
    paths: Vec<MissingPath>,
}

struct MissingPath {
    path: String,
    detail: Option<&'static str>,
}

async fn files_check(
    db: &InstalledDb,
    root_dir: &Path,
    only_parts: Option<&[String]>,
) -> Result<(usize, Vec<CheckIssue>)> {
    crate::cli_action!("Checking", "deployed file existence");

    let parts: Vec<InstalledPart> = match only_parts {
        Some(names) => {
            let mut selected = Vec::with_capacity(names.len());
            for name in names {
                match db.get_part(name).await? {
                    Some(p) => selected.push(p),
                    None => return Err(WrightError::PartNotFound(name.clone())),
                }
            }
            selected
        }
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
        let mut missing_paths: Vec<MissingPath> = Vec::new();

        for f in &files {
            let abs = root_dir.join(f.path.trim_start_matches('/'));
            match f.file_type {
                FileType::File => {
                    if !abs.is_file() {
                        let detail = if abs.exists() {
                            Some("wrong type")
                        } else {
                            None
                        };
                        missing_paths.push(MissingPath {
                            path: abs.display().to_string(),
                            detail,
                        });
                    }
                }
                FileType::Symlink => {
                    if !abs.is_symlink() {
                        let detail = if abs.exists() {
                            Some("expected symlink")
                        } else {
                            None
                        };
                        missing_paths.push(MissingPath {
                            path: abs.display().to_string(),
                            detail,
                        });
                    }
                }
                FileType::Directory => {
                    if !abs.is_dir() {
                        let detail = if abs.exists() {
                            Some("expected directory")
                        } else {
                            None
                        };
                        missing_paths.push(MissingPath {
                            path: abs.display().to_string(),
                            detail,
                        });
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
        return Ok((0, Vec::new()));
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
        let render = |mp: &MissingPath| {
            let suffix = mp.detail.map(|d| format!(" ({})", d)).unwrap_or_default();
            format!("{}: {}{}", pm.part_name, mp.path, suffix)
        };
        if count <= 5 {
            for mp in &pm.paths {
                lines.push(render(mp));
            }
        } else {
            for mp in pm.paths.iter().take(3) {
                lines.push(render(mp));
            }
            lines.push(format!("{}: ... and {} more", pm.part_name, count - 3));
        }
    }
    emit_bullets(&lines);

    let found = report
        .missing
        .into_iter()
        .flat_map(|pm| {
            pm.paths.into_iter().map(move |mp| CheckIssue::MissingFile {
                part: pm.part_name.clone(),
                path: mp.path,
                detail: mp.detail.map(str::to_string),
            })
        })
        .collect();

    Ok((total_missing, found))
}

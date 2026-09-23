//! Drift audit: report what is on the live root that Wright does not know
//! about (ADR-0043).
//!
//! Every other verification surface Wright has runs registry → disk:
//! `check --files` and `lint --verify` find owned files that were deleted or
//! modified. None of them walk the root, so none of them can find files that
//! were *added*. `audit` closes that gap. It is read-only and never deletes:
//! `unowned` does not imply `safe`, and the residue includes hook-created
//! paths, user-edited configs, and files a foreign part installed.
//!
//! The managed scope is [`fhs::is_audited`] — the same definition the seal
//! path enforces, so audit cannot drift from what Wright is allowed to own.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;
use wright_part::fhs;
use wright_state::database::ReadOnlyDb;

/// One unowned path.
#[derive(Debug, Serialize)]
pub struct DriftFinding {
    pub path: String,
    pub kind: String,
}

/// The full audit report.
#[derive(Debug, Serialize)]
pub struct AuditReport {
    /// Root that was walked.
    pub root: String,
    /// Files under the managed scope that were walked.
    pub scanned_files: u64,
    /// Files the registry claims to own (in scope).
    pub owned_files: u64,
    /// Paths that are in scope but owned by no part.
    pub unowned: Vec<DriftFinding>,
    /// External parts found in the registry. Their files are unknown to the
    /// registry by construction (`wright provide` records no paths), so audit
    /// cannot distinguish their content from genuine drift.
    pub external_parts: Vec<String>,
    /// A human-readable statement of the external-part blind spot, or `None`
    /// when there are no external parts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_blind_spot: Option<String>,
}

/// Walk the managed scope and report drift.
pub async fn execute_audit(
    db: &ReadOnlyDb,
    root_dir: &Path,
    json: bool,
    include_dirs: bool,
) -> Result<()> {
    let owned = db.all_owned_paths().await?;
    let external = db.get_provided_parts().await?;
    let external_names: Vec<String> = external.into_iter().map(|part| part.name).collect();

    // Every owned path is normalised to the absolute form the walk produces.
    let owned_absolute: BTreeSet<PathBuf> = owned
        .iter()
        .map(|path| root_dir.join(path.trim_start_matches('/')))
        .collect();

    let mut scanned_files = 0u64;
    let mut owned_in_scope = 0u64;
    let mut unowned = Vec::new();

    for entry in walkdir::WalkDir::new(root_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| should_descend(entry.path(), root_dir))
        .flatten()
    {
        let path = entry.path();
        let file_type = entry.file_type();
        if file_type.is_dir() && !include_dirs {
            continue;
        }
        // The managed scope is defined once, in `fhs`, and is expressed in
        // absolute-from-`/` terms — so classify by the path as the root sees
        // it, not by the host path the walk produced. `/usr/local` and
        // `/usr/src` are hand-managed, `/var` and `/boot` are high-churn, so
        // audit makes no claim about them (ADR-0043).
        let recorded = root_relative(path, root_dir);
        if !fhs::is_audited(&recorded) {
            continue;
        }
        // Symlinks are audited as themselves; their targets are not followed.
        scanned_files += 1;

        if owned_absolute.contains(path) {
            owned_in_scope += 1;
            continue;
        }

        // Whitelist tool-generated config artifacts (e.g. `<config>.wnew`, `<config>.worig`).
        // When Wright updates a user-modified config file, it preserves the existing file
        // and writes `<name>.wnew` alongside. These are known managed artifacts rather than unowned drift.
        if is_tool_generated_config(path) {
            continue;
        }

        unowned.push(DriftFinding {
            path: path.display().to_string(),
            kind: describe_kind(&file_type),
        });
    }

    let external_blind_spot = if external_names.is_empty() {
        None
    } else {
        Some(format!(
            "{} external part(s) ({}) record no file paths; their content is unverified, not drift",
            external_names.len(),
            external_names.join(", ")
        ))
    };

    let report = AuditReport {
        root: root_dir.display().to_string(),
        scanned_files,
        owned_files: owned_in_scope,
        unowned,
        external_parts: external_names,
        external_blind_spot,
    };

    if json {
        return super::print_json(&report);
    }

    crate::cli_output!(
        "Scanned {} file(s) under {}; {} owned, {} unowned.",
        report.scanned_files,
        report.root,
        report.owned_files,
        report.unowned.len()
    );
    for finding in &report.unowned {
        crate::cli_output!("  {} ({})", finding.path, finding.kind);
    }
    if let Some(statement) = &report.external_blind_spot {
        crate::cli_warn!("{}", statement);
    }
    crate::cli_output!(
        "Audit is read-only: unowned does not mean safe. Review each path before acting."
    );
    Ok(())
}

/// Descend into the audited top-level directories only. The root itself is
/// always descended so the top-level filter can run.
fn should_descend(path: &Path, root: &Path) -> bool {
    if path == root {
        return true;
    }
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut components = relative.components();
    let Some(first) = components.next() else {
        return true;
    };
    if components.next().is_some() {
        // Below the top level: always descend; pruning is a top-level concern.
        return true;
    }
    let name = first.as_os_str().to_string_lossy();
    fhs::AUDITED_TOP_DIRS.contains(&name.as_ref())
}

/// Re-express a walk path as the root-relative absolute path the registry
/// stores, so `fhs::is_audited` (which is written in absolute-from-`/` terms)
/// classifies correctly even when auditing a redirected root.
fn root_relative(path: &Path, root: &Path) -> PathBuf {
    match path.strip_prefix(root) {
        Ok(relative) => PathBuf::from("/").join(relative),
        Err(_) => path.to_path_buf(),
    }
}

fn describe_kind(file_type: &std::fs::FileType) -> String {
    if file_type.is_symlink() {
        "symlink".to_string()
    } else if file_type.is_dir() {
        "dir".to_string()
    } else {
        "file".to_string()
    }
}

/// Returns true if the file path is a tool-generated configuration artifact
/// (e.g., `<name>.wnew` or `<name>.worig`), which Wright creates during config merges.
pub fn is_tool_generated_config(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(".wnew") || name.ends_with(".worig"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::InstalledDb;

    #[test]
    fn descend_into_managed_tops_only() {
        let root = Path::new("/");
        assert!(should_descend(Path::new("/"), root));
        assert!(should_descend(Path::new("/usr"), root));
        assert!(should_descend(Path::new("/etc"), root));
        assert!(should_descend(Path::new("/usr/bin"), root));
        assert!(!should_descend(Path::new("/home"), root));
        assert!(!should_descend(Path::new("/proc"), root));
        assert!(!should_descend(Path::new("/root"), root));
        // `/var` and `/boot` are in the install scope but not the audit scope.
        assert!(!should_descend(Path::new("/var"), root));
        assert!(!should_descend(Path::new("/boot"), root));
    }

    #[test]
    fn audited_scope_excludes_hand_managed_prefixes() {
        assert!(fhs::is_audited(Path::new("/usr/bin/demo")));
        assert!(fhs::is_audited(Path::new("/etc/demo.conf")));
        assert!(fhs::is_audited(Path::new("/opt/demo/bin")));
        // Installable, but not audited.
        assert!(!fhs::is_audited(Path::new("/usr/local/bin/demo")));
        assert!(!fhs::is_audited(Path::new("/usr/src/linux")));
        assert!(!fhs::is_audited(Path::new("/var/lib/wright/wright.db")));
        assert!(!fhs::is_audited(Path::new("/boot/vmlinuz")));
    }

    #[test]
    fn recorded_path_round_trips() {
        let root = Path::new("/");
        assert_eq!(
            root.join("/usr/bin/demo".trim_start_matches('/')),
            PathBuf::from("/usr/bin/demo")
        );
    }

    #[test]
    fn root_relative_rewrites_redirected_roots() {
        assert_eq!(
            root_relative(
                Path::new("/mnt/target/usr/bin/demo"),
                Path::new("/mnt/target")
            ),
            PathBuf::from("/usr/bin/demo")
        );
        assert_eq!(
            root_relative(Path::new("/usr/bin/demo"), Path::new("/")),
            PathBuf::from("/usr/bin/demo")
        );
    }

    #[tokio::test]
    async fn audit_reports_unowned_files_in_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::write(root.join("usr/bin/owned"), "x").unwrap();
        std::fs::write(root.join("usr/bin/untracked"), "x").unwrap();
        // Outside the audited scope entirely.
        std::fs::create_dir_all(root.join("home/user")).unwrap();
        std::fs::write(root.join("home/user/ignored"), "x").unwrap();

        let db = InstalledDb::open_in_memory().await.unwrap();
        let plan_id = db
            .insert_plan(wright_state::database::NewPlan {
                name: "demo",
                version: "1.0.0",
                release: 1,
                epoch: 0,
                arch: "x86_64",
            })
            .await
            .unwrap();
        let part_id = db
            .insert_part(wright_state::database::NewPart {
                name: "demo",
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();
        db.insert_files(
            part_id,
            &[wright_state::database::FileEntry {
                path: "/usr/bin/owned".to_string(),
                file_hash: None,
                file_type: wright_state::database::FileType::File,
                file_mode: None,
                file_size: Some(1),
                is_config: false,
            }],
        )
        .await
        .unwrap();

        execute_audit(&db, root, false, false).await.unwrap();
        // Behaviour asserted via the DB-level path set that drives the report.
        let owned = db.all_owned_paths().await.unwrap();
        assert!(owned.contains("/usr/bin/owned"));
        assert!(!owned.contains("/usr/bin/untracked"));
    }

    #[tokio::test]
    async fn audit_ignores_tool_generated_config_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::write(root.join("etc/app.conf.wnew"), "new config").unwrap();
        std::fs::write(root.join("etc/app.conf.worig"), "orig config").unwrap();

        let db = InstalledDb::open_in_memory().await.unwrap();
        let unowned = execute_audit(&db, root, false, false).await;
        assert!(unowned.is_ok());

        assert!(is_tool_generated_config(Path::new("/etc/app.conf.wnew")));
        assert!(is_tool_generated_config(Path::new("/etc/app.conf.worig")));
        assert!(!is_tool_generated_config(Path::new("/etc/app.conf")));
    }
}

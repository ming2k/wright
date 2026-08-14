use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};

use crate::config::GlobalConfig;
use crate::identify::Identifier;
use crate::resolve::{plan_search_dirs, resolve_targets};
use crate::util::stdin::collect_stdin_args;
use wright_part::store::{LocalPartStore, pick_latest};
use wright_plan::manifest::PlanManifest;
use wright_state::database::{InstalledDb, SessionContext};

fn looks_like_archive_path(arg: &str) -> bool {
    arg.ends_with(".wright.tar.zst")
}

/// Store-side resolution for one merge target, following the universal
/// identifier grammar (see [`crate::identify`]):
///
/// - `plan:output` pins the originating plan and picks the latest version
///   that plan sealed.
/// - A bare name matches archives by output name; when the matches come
///   from several distinct plans the target is ambiguous and resolution
///   fails, naming the absolute `plan:output` forms.
/// - `plan:*` and unmatched bare names return `None`, leaving resolution
///   to the plan-directory fallback.
async fn resolve_store_archive(
    part_store: &LocalPartStore,
    ident: &Identifier,
) -> Result<Option<(PathBuf, String)>> {
    match ident {
        Identifier::Output { plan, output } => {
            let candidates = part_store
                .resolve_all_from_plan(output, plan)
                .await
                .map_err(|e| WrightError::context(format!("resolve part {}", output), e))?;
            let latest = pick_latest(&candidates).ok_or_else(|| {
                WrightError::PartNotFound(format!(
                    "no archive for '{}:{}' in the part store",
                    plan, output
                ))
            })?;
            Ok(Some((latest.path.clone(), latest.name.clone())))
        }
        Identifier::Bare(name) => {
            let all = part_store
                .resolve_all(name)
                .await
                .map_err(|e| WrightError::context(format!("resolve part {}", name), e))?;
            if all.is_empty() {
                return Ok(None);
            }
            let mut plans: Vec<&str> = all.iter().map(|p| p.plan_name.as_str()).collect();
            plans.sort_unstable();
            plans.dedup();
            if plans.len() > 1 {
                let forms = plans
                    .iter()
                    .map(|plan| format!("{}:{}", plan, name))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(WrightError::AmbiguousTarget(format!(
                    "'{}' matches part archives from multiple plans; use one of: {}",
                    name, forms
                )));
            }
            let latest = pick_latest(&all).expect("at least one candidate");
            Ok(Some((latest.path.clone(), latest.name.clone())))
        }
        Identifier::Plan(_) => Ok(None),
    }
}

/// Resolve a merge target as a plan name/directory: deploy every output of
/// the plan, pinned to that plan so a foreign archive that merely shares
/// the output name is never picked up.
async fn resolve_plan_outputs(
    arg: &str,
    config: &GlobalConfig,
    part_store: &LocalPartStore,
    paths: &mut Vec<PathBuf>,
    explicit: &mut HashSet<String>,
) -> Result<()> {
    let plan_path = PathBuf::from(arg);
    let manifest = if plan_path.is_dir() {
        PlanManifest::from_file(&plan_path.join("plan.toml"))
            .map_err(|e| WrightError::context(format!("read plan {}", arg), e))?
    } else {
        let plan_dirs = plan_search_dirs(config);
        let index = wright_plan::discovery::PlanIndex::discover(&plan_dirs)?;
        let resolved = resolve_targets(&[arg.to_string()], &index, &plan_dirs)?;
        if resolved.is_empty() {
            return Err(WrightError::PartNotFound(format!(
                "target not found: {}",
                arg
            )));
        }
        let plan_path = resolved.into_iter().next().unwrap();
        PlanManifest::from_file(&plan_path)
            .map_err(|e| WrightError::context(format!("read plan {}", arg), e))?
    };

    let part_names = match manifest.outputs {
        Some(wright_plan::manifest::OutputConfig::Multi(ref parts)) => {
            parts.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()
        }
        _ => vec![manifest.metadata.name.clone()],
    };

    for pn in part_names {
        let candidates = part_store
            .resolve_all_from_plan(&pn, &manifest.metadata.name)
            .await
            .map_err(|e| {
                WrightError::context(format!("resolve part {} from plan {}", pn, arg), e)
            })?;
        let resolved = pick_latest(&candidates).ok_or_else(|| {
            WrightError::PartNotFound(format!("part {} not found in parts_dir", pn))
        })?;
        paths.push(resolved.path.clone());
        explicit.insert(pn);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_merge(
    targets: Vec<String>,
    force: bool,
    nodeps: bool,
    path: bool,
    dry_run: bool,
    _config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    part_store: &LocalPartStore,
) -> Result<()> {
    let targets = collect_stdin_args(targets)?;
    use std::io::IsTerminal;
    if targets.is_empty() {
        if !std::io::stdin().is_terminal() {
            if path {
                return Err(WrightError::ForgeError(
                    "no archive paths received from stdin; did the build succeed?".into(),
                ));
            }
            return Err(WrightError::ForgeError(
                "no merge targets received from stdin; did the resolve succeed?".into(),
            ));
        }
        if path {
            return Err(WrightError::ForgeError(
                "no archive paths specified (pass paths as arguments or via stdin)".into(),
            ));
        }
        return Err(WrightError::ForgeError(
        "no merge targets specified (pass plan names/directories, or use --path for archive paths)".into()
    ));
    }

    if !path {
        for arg in &targets {
            if looks_like_archive_path(arg) {
                return Err(WrightError::ForgeError(format!(
                    "'{}' looks like an archive path; use `wright merge --path {}`",
                    arg, arg
                )));
            }
        }
    }

    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::context("open database", e))?;

    // ── Resolution phase (read-only) ────────────────────────────────
    // Turn every argument into a concrete archive path. `explicit` records
    // which part names the user asked for directly.
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut explicit: HashSet<String> = HashSet::new();

    if path {
        for arg in &targets {
            let p = PathBuf::from(arg);
            if !p.is_file() {
                return Err(WrightError::ForgeError(format!(
                    "archive path not found: {}",
                    p.display()
                )));
            }
            paths.push(p);
        }
    } else {
        for arg in &targets {
            let ident = Identifier::parse(arg)?;
            if let Some((path, name)) = resolve_store_archive(part_store, &ident).await? {
                paths.push(path);
                explicit.insert(name);
                continue;
            }

            // No store archive matched: treat the target as a plan
            // name/directory and deploy every output of that plan.
            let plan_arg = match &ident {
                Identifier::Plan(plan) => plan.as_str(),
                _ => arg.as_str(),
            };
            resolve_plan_outputs(plan_arg, _config, part_store, &mut paths, &mut explicit).await?;
        }
    }

    if dry_run {
        crate::outln!("[dry-run] merge -> {}", root_dir.display());
        crate::outln!("[dry-run] would deploy {} archive(s):", paths.len());
        for p in &paths {
            crate::outln!("  {}", p.display());
        }
        return Ok(());
    }

    // ── Deploy phase ────────────────────────────────────────────────
    let command_str = format!("merge {}", targets.join(" "));
    let tx_id = wright_state::delivery::begin_delivery(&db, &command_str).await?;
    let session = SessionContext {
        id: format!(
            "{:x}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        command: command_str,
    };

    let result = if path {
        crate::transaction::deploy_parts(
            &db,
            &paths,
            root_dir,
            part_store,
            force,
            nodeps,
            true,
            session.clone(),
        )
        .await
    } else {
        crate::transaction::deploy_parts_with_explicit_targets(
            &db,
            &paths,
            &explicit,
            root_dir,
            part_store,
            force,
            nodeps,
            None,
            true,
            session.clone(),
        )
        .await
    };

    if let Err(e) = result {
        let _ = wright_state::delivery::rollback_delivery(&db, tx_id).await;
        return Err(WrightError::context("merge", e));
    }

    wright_state::delivery::complete_delivery(&db, tx_id).await?;
    let _ = wright_state::delivery::cleanup_delivery(&db, tx_id).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_part::archive::{PartHooks, PartSpec, PlanMetadata, Provenance, write_part};

    /// Seal a `prism-<version>` archive for `plan` into the plan's
    /// subdirectory of `parts_dir`, mirroring the current layout.
    fn seal_prism(parts_dir: &Path, plan: &str, version: &str) -> PathBuf {
        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/lib")).unwrap();
        std::fs::write(staging.path().join("usr/lib/prism"), plan).unwrap();
        let output_dir = parts_dir.join(plan);
        std::fs::create_dir_all(&output_dir).unwrap();
        let spec = PartSpec {
            archive_name: format!("prism-{version}-1-x86_64.wright.tar.zst"),
            name: "prism".to_string(),
            runtime_deps: Vec::new(),
            replaces: Vec::new(),
            conflicts: Vec::new(),
            backup_files: Vec::new(),
            plan: PlanMetadata {
                name: plan.to_string(),
                version: version.to_string(),
                release: 1,
                epoch: 0,
                arch: "x86_64".to_string(),
            },
            provenance: Provenance {
                plan_checksum: None,
                source_checksums: Vec::new(),
                wright_version: env!("CARGO_PKG_VERSION").to_string(),
                isolation: "strict".to_string(),
            },
            plan_source: None,
            hooks: PartHooks::default(),
        };
        write_part(staging.path(), &spec, &output_dir).unwrap()
    }

    fn test_store(parts_dir: &Path) -> LocalPartStore {
        let mut store = LocalPartStore::new();
        store.add_search_dir(parts_dir.to_path_buf());
        store
    }

    #[tokio::test]
    async fn bare_name_matching_one_plan_picks_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();
        seal_prism(&parts_dir, "optics", "1.0.0");
        let new = seal_prism(&parts_dir, "optics", "2.0.0");
        let store = test_store(&parts_dir);

        let (path, name) = resolve_store_archive(&store, &Identifier::parse("prism").unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(path, new);
        assert_eq!(name, "prism");
    }

    #[tokio::test]
    async fn bare_name_matching_multiple_plans_is_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();
        seal_prism(&parts_dir, "optics", "1.0.0");
        seal_prism(&parts_dir, "prism", "9.0.537");
        let store = test_store(&parts_dir);

        let err = resolve_store_archive(&store, &Identifier::parse("prism").unwrap())
            .await
            .unwrap_err();
        let WrightError::AmbiguousTarget(msg) = err else {
            panic!("expected ambiguous target, got: {}", err);
        };
        assert!(msg.contains("optics:prism"), "message names form: {}", msg);
        assert!(msg.contains("prism:prism"), "message names form: {}", msg);
    }

    #[tokio::test]
    async fn qualified_target_pins_the_plan() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();
        let optics = seal_prism(&parts_dir, "optics", "0.0.14");
        // A foreign plan's much newer same-named archive must not win.
        seal_prism(&parts_dir, "prism", "9.0.537");
        let store = test_store(&parts_dir);

        let (path, name) =
            resolve_store_archive(&store, &Identifier::parse("optics:prism").unwrap())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(path, optics);
        assert_eq!(name, "prism");

        let err = resolve_store_archive(&store, &Identifier::parse("nope:prism").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, WrightError::PartNotFound(_)));

        // `plan:*` never resolves through the store: it falls back to
        // plan-directory resolution.
        assert!(
            resolve_store_archive(&store, &Identifier::parse("optics:*").unwrap())
                .await
                .unwrap()
                .is_none()
        );
    }
}

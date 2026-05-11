use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};

use super::apply::collect_install_args;
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::part::store::LocalPartStore;
use crate::plan::manifest::PlanManifest;
use crate::planning::{plan_search_dirs, resolve_targets};

fn looks_like_archive_path(arg: &str) -> bool {
    arg.ends_with(".wright.tar.zst")
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_install(
    parts: Vec<String>,
    force: bool,
    nodeps: bool,
    path: bool,
    _config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    part_store: &LocalPartStore,
) -> Result<()> {
    let parts = collect_install_args(parts)?;
    use std::io::IsTerminal;
    if parts.is_empty() {
        if !std::io::stdin().is_terminal() {
        if path {
            return Err(WrightError::BuildError("no archive paths received from stdin; did the build succeed?".into()));
        }
        return Err(WrightError::BuildError("no install targets received from stdin; did the resolve succeed?".into()));
    }
    if path {
        return Err(WrightError::BuildError("no archive paths specified (pass paths as arguments or via stdin)".into()));
    }
    return Err(WrightError::BuildError(
        "no install targets specified (pass plan names/directories, or use --path for archive paths)".into()
    ));
    }

    if !path {
        for arg in &parts {
            if looks_like_archive_path(arg) {
                return Err(WrightError::BuildError(format!(
                    "'{}' looks like an archive path; use `wright install --path {}`",
                    arg, arg
                )));
            }
        }
    }

    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open database: {}", e)))?;

    if path {
        let mut paths: Vec<PathBuf> = Vec::new();
        for arg in &parts {
            let p = PathBuf::from(arg);
            if !p.is_file() {
                return Err(WrightError::BuildError(format!("archive path not found: {}", p.display())));
            }
            paths.push(p);
        }

        crate::transaction::install_parts(&db, &paths, root_dir, part_store, force, nodeps,
        )
        .await
        .map_err(|e| WrightError::InstallError(format!("install archives: {}", e)))?;
    } else {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut explicit: HashSet<String> = HashSet::new();

        for arg in &parts {
            // Try resolving as a part name first.
            if let Some(resolved) = part_store
                .resolve(arg)
                .await
                .map_err(|e| WrightError::PartError(format!("resolve part {}: {}", arg, e)))?
            {
                paths.push(resolved.path);
                explicit.insert(resolved.name);
                continue;
            }

            // Fall back to treating as a plan name/directory.
            let plan_path = PathBuf::from(arg);
                let manifest = if plan_path.is_dir() {
                    PlanManifest::from_file(&plan_path.join("plan.toml"))
                        .map_err(|e| WrightError::BuildError(format!("read plan {}: {}", arg, e)))?
                } else {
                    let plan_dirs = plan_search_dirs(_config);
                    let index = crate::plan::discovery::PlanIndex::discover(&plan_dirs)?;
                    let resolved = resolve_targets(&[arg.clone()], &index, &plan_dirs)?;
                    if resolved.is_empty() {
                        return Err(WrightError::PartNotFound(format!("target not found: {}", arg)));
                    }
                    let plan_path = resolved.into_iter().next().unwrap();
                    PlanManifest::from_file(&plan_path)
                        .map_err(|e| WrightError::BuildError(format!("read plan {}: {}", arg, e)))?
                };

            let part_names = match manifest.outputs {
                Some(crate::plan::manifest::OutputConfig::Multi(ref parts)) => {
                    parts.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()
                }
                _ => vec![manifest.metadata.name.clone()],
            };

            for pn in part_names {
                let resolved = part_store
                    .resolve(&pn)
                    .await
                    .map_err(|e| WrightError::PartError(format!("resolve part {} from plan {}: {}", pn, arg, e)))?
                    .ok_or_else(|| WrightError::PartNotFound(format!("part {} not found in parts_dir", pn)))?;
                paths.push(resolved.path);
                explicit.insert(pn);
            }
        }

        crate::transaction::install_parts_with_explicit_targets(
            &db,
            &paths,
            &explicit,
            root_dir,
            part_store,
            force,
            nodeps,
        )
        .await
        .map_err(|e| WrightError::InstallError(format!("install targets: {}", e)))?;
    }

    Ok(())
}

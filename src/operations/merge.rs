use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};

use crate::config::GlobalConfig;
use crate::util::stdin::collect_stdin_args;
use crate::database::{InstalledDb, SessionContext};
use crate::part::store::LocalPartStore;
use crate::plan::manifest::PlanManifest;
use crate::resolve::{plan_search_dirs, resolve_targets};

fn looks_like_archive_path(arg: &str) -> bool {
    arg.ends_with(".wright.tar.zst")
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_merge(
    parts: Vec<String>,
    force: bool,
    nodeps: bool,
    path: bool,
    _config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    part_store: &LocalPartStore,
) -> Result<()> {
    let parts = collect_stdin_args(parts)?;
    use std::io::IsTerminal;
    if parts.is_empty() {
        if !std::io::stdin().is_terminal() {
            if path {
                return Err(WrightError::ForgeError(
                    "no archive paths received from stdin; did the build succeed?".into(),
                ));
            }
            return Err(WrightError::ForgeError(
                "no install targets received from stdin; did the resolve succeed?".into(),
            ));
        }
        if path {
            return Err(WrightError::ForgeError(
                "no archive paths specified (pass paths as arguments or via stdin)".into(),
            ));
        }
        return Err(WrightError::ForgeError(
        "no install targets specified (pass plan names/directories, or use --path for archive paths)".into()
    ));
    }

    if !path {
        for arg in &parts {
            if looks_like_archive_path(arg) {
                return Err(WrightError::ForgeError(format!(
                    "'{}' looks like an archive path; use `wright install --path {}`",
                    arg, arg
                )));
            }
        }
    }

    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open database: {}", e)))?;

    let command_str = format!("merge {}", parts.join(" "));
    let tx_id = crate::delivery::begin_delivery(&db, &command_str).await?;
    let session = SessionContext {
        id: format!(
            "{:x}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        command: command_str,
    };

    if path {
        let mut paths: Vec<PathBuf> = Vec::new();
        for arg in &parts {
            let p = PathBuf::from(arg);
            if !p.is_file() {
                return Err(WrightError::ForgeError(format!(
                    "archive path not found: {}",
                    p.display()
                )));
            }
            paths.push(p);
        }

        let result = crate::transaction::deploy_parts(
            &db,
            &paths,
            root_dir,
            part_store,
            force,
            nodeps,
            session.clone(),
        )
        .await;

        match result {
            Ok(()) => {}
            Err(e) => {
                let _ = crate::delivery::rollback_delivery(&db, tx_id).await;
                return Err(WrightError::DeployError(format!("install archives: {}", e)));
            }
        }
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
                    .map_err(|e| WrightError::ForgeError(format!("read plan {}: {}", arg, e)))?
            } else {
                let plan_dirs = plan_search_dirs(_config);
                let index = crate::plan::discovery::PlanIndex::discover(&plan_dirs)?;
                let resolved = resolve_targets(&[arg.to_string()], &index, &plan_dirs)?;
                if resolved.is_empty() {
                    return Err(WrightError::PartNotFound(format!(
                        "target not found: {}",
                        arg
                    )));
                }
                let plan_path = resolved.into_iter().next().unwrap();
                PlanManifest::from_file(&plan_path)
                    .map_err(|e| WrightError::ForgeError(format!("read plan {}: {}", arg, e)))?
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
                    .map_err(|e| {
                        WrightError::PartError(format!(
                            "resolve part {} from plan {}: {}",
                            pn, arg, e
                        ))
                    })?
                    .ok_or_else(|| {
                        WrightError::PartNotFound(format!("part {} not found in parts_dir", pn))
                    })?;
                paths.push(resolved.path);
                explicit.insert(pn);
            }
        }

        let result = crate::transaction::deploy_parts_with_explicit_targets(
            &db,
            &paths,
            &explicit,
            root_dir,
            part_store,
            force,
            nodeps,
            session.clone(),
        )
        .await;

        match result {
            Ok(()) => {}
            Err(e) => {
                let _ = crate::delivery::rollback_delivery(&db, tx_id).await;
                return Err(WrightError::DeployError(format!("install targets: {}", e)));
            }
        }
    }

    crate::delivery::complete_delivery(&db, tx_id).await?;
    let _ = crate::delivery::cleanup_delivery(&db, tx_id).await;

    Ok(())
}

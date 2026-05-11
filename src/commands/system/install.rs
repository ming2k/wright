use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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
                anyhow::bail!("no archive paths received from stdin; did the build succeed?");
            }
            anyhow::bail!("no install targets received from stdin; did the resolve succeed?");
        }
        if path {
            anyhow::bail!("no archive paths specified (pass paths as arguments or via stdin)");
        }
        anyhow::bail!(
            "no install targets specified (pass plan names/directories, or use --path for archive paths)"
        );
    }

    if !path {
        for arg in &parts {
            if looks_like_archive_path(arg) {
                anyhow::bail!(
                    "'{}' looks like an archive path; use `wright install --path {}`",
                    arg,
                    arg
                );
            }
        }
    }

    let db = InstalledDb::open(db_path)
        .await
        .context("open database")?;

    if path {
        let mut paths: Vec<PathBuf> = Vec::new();
        for arg in &parts {
            let p = PathBuf::from(arg);
            if !p.is_file() {
                anyhow::bail!("archive path not found: {}", p.display());
            }
            paths.push(p);
        }

        crate::transaction::install_parts(&db, &paths, root_dir, part_store, force, nodeps,
        )
        .await
        .context("install archives")?;
    } else {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut explicit: HashSet<String> = HashSet::new();

        for arg in &parts {
            // Try resolving as a part name first.
            if let Some(resolved) = part_store
                .resolve(arg)
                .await
                .with_context(|| format!("resolve part {}", arg))?
            {
                paths.push(resolved.path);
                explicit.insert(resolved.name);
                continue;
            }

            // Fall back to treating as a plan name/directory.
            let plan_path = PathBuf::from(arg);
            let manifest = if plan_path.is_dir() {
                PlanManifest::from_file(&plan_path.join("plan.toml"))
                    .with_context(|| format!("read plan {}", arg))?
            } else {
                let plan_dirs = plan_search_dirs(_config);
                let index = crate::plan::discovery::PlanIndex::discover(&plan_dirs)?;
                let resolved = resolve_targets(&[arg.clone()], &index, &plan_dirs)?;
                if resolved.is_empty() {
                    anyhow::bail!("target not found: {}", arg);
                }
                let plan_path = resolved.into_iter().next().unwrap();
                PlanManifest::from_file(&plan_path)
                    .with_context(|| format!("read plan {}", arg))?
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
                    .with_context(|| format!("resolve part {} from plan {}", pn, arg))?
                    .ok_or_else(|| anyhow::anyhow!("part {} not found in parts_dir", pn))?;
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
        .context("install targets")?;
    }

    Ok(())
}

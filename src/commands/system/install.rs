use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use super::apply::collect_install_args;
use crate::commands::workflow_run::{drive_command, DriveOptions};
use crate::config::GlobalConfig;
use crate::part::store::LocalPartStore;
use crate::workflow::builders::{
    build_install_archives_workflow, build_install_targets_workflow,
};

fn looks_like_archive_path(arg: &str) -> bool {
    arg.ends_with(".wright.tar.zst")
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_install(
    parts: Vec<String>,
    force: bool,
    nodeps: bool,
    path: bool,
    config: &GlobalConfig,
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

    let part_store_arc = Arc::new((*part_store).clone());

    let spec = if path {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut explicit: Vec<String> = Vec::new();
        for arg in &parts {
            let p = PathBuf::from(arg);
            if !p.is_file() {
                anyhow::bail!("archive path not found: {}", p.display());
            }
            let info = crate::part::archive::read_partinfo(&p)
                .with_context(|| format!("read PARTINFO from {}", p.display()))?;
            explicit.push(info.name);
            paths.push(p);
        }
        build_install_archives_workflow(
            paths,
            explicit,
            root_dir.to_path_buf(),
            part_store_arc,
            force,
            nodeps,
        )
    } else {
        build_install_targets_workflow(
            config,
            parts,
            root_dir.to_path_buf(),
            part_store_arc,
            force,
            nodeps,
        )
    }
    .map_err(|e| anyhow::anyhow!("install workflow: {}", e))?;

    drive_command(
        spec,
        DriveOptions {
            config,
            db_path,
            fresh: false,
            quiet: false,
        },
    )
    .await
    .map(|_| ())
}

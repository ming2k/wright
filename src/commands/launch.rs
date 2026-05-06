use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use crate::cli::launch::LaunchArgs;
use crate::commands::workflow_run::{drive_command, DriveOptions};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::part::pack;
use crate::part::store::LocalPartStore;
use crate::workflow::builders::build_launch_pack_workflow;

pub async fn execute_launch(
    args: LaunchArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    if root_dir == Path::new("/") {
        anyhow::bail!(
            "wright launch refuses to fill `/`; pass --root <PATH> pointing at a mounted target root"
        );
    }
    ensure_target_skeleton(root_dir)
        .with_context(|| format!("failed to prepare target root {}", root_dir.display()))?;

    if let Some(plans_dir) = args.plans.clone() {
        return launch_from_plans(args, plans_dir, config, db_path, root_dir, verbose, quiet).await;
    }

    let pack_path = match (args.pack.clone(), args.profile.clone()) {
        (Some(p), _) => p,
        (None, Some(profile)) => resolve_profile(&profile, &config.general.pack_dirs)
            .with_context(|| format!("failed to resolve profile '{}'", profile))?,
        (None, None) => {
            anyhow::bail!("wright launch needs a pack file, --plans, or --profile; nothing to do")
        }
    };

    launch_from_pack(
        pack_path,
        args.dry_run,
        args.force,
        config,
        db_path,
        root_dir,
    )
    .await
}

async fn launch_from_pack(
    pack_path: PathBuf,
    dry_run: bool,
    force: bool,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
) -> Result<()> {
    // Display header up front so the user sees what's about to happen.
    let manifest = pack::read_manifest(&pack_path)
        .with_context(|| format!("read pack {}", pack_path.display()))?;
    println!(
        "launching pack {} {} into {}",
        manifest.pack.name,
        manifest.pack.version,
        root_dir.display()
    );
    info!(
        "pack carries {} part(s), {} assumption(s){}",
        manifest.parts.len(),
        manifest.assumes.len(),
        if manifest.config.is_some() {
            ", config block"
        } else {
            ""
        }
    );

    if dry_run {
        println!();
        println!("[dry-run] would install {} part(s):", manifest.parts.len());
        for p in &manifest.parts {
            let origin = match p.origin {
                pack::PackOrigin::Manual => "manual",
                pack::PackOrigin::Dependency => "dep",
            };
            println!("  {:<10} {}", origin, p.file);
        }
        if !manifest.assumes.is_empty() {
            println!();
            println!(
                "[dry-run] would assume {} external(s):",
                manifest.assumes.len()
            );
            for a in &manifest.assumes {
                println!("  {} {}", a.name, a.version);
            }
        }
        return Ok(());
    }

    // Pre-register external assumptions; these aren't part of the workflow
    // because they're idempotent DB-only writes and the workflow shouldn't
    // own external-state declarations.
    let db = InstalledDb::open(db_path)
        .await
        .context("failed to open target database")?;
    for assume in &manifest.assumes {
        db.assume_part(&assume.name, &assume.version)
            .await
            .with_context(|| format!("failed to assume {}", assume.name))?;
    }
    drop(db);

    let part_store = part_store_for_target(root_dir);
    let staging_root = root_dir.join("var/lib/wright/pack-staging");
    std::fs::create_dir_all(&staging_root)
        .with_context(|| format!("create {}", staging_root.display()))?;

    let spec = build_launch_pack_workflow(
        pack_path.clone(),
        root_dir.to_path_buf(),
        staging_root,
        Arc::new(part_store),
        force,
    )
    .map_err(|e| anyhow::anyhow!("launch workflow: {}", e))?;

    drive_command(
        spec,
        DriveOptions {
            config,
            db_path,
            fresh: false,
            quiet: false,
        },
    )
    .await?;

    println!("launched {} -> {}", pack_path.display(), root_dir.display());
    Ok(())
}

async fn launch_from_plans(
    args: LaunchArgs,
    plans_dir: PathBuf,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    if args.plan_targets.is_empty() {
        anyhow::bail!("--plans needs one or more plan names to launch");
    }

    let mut launch_config = config.clone();
    launch_config.general.extra_plans_dirs.push(plans_dir);
    let part_store = crate::commands::setup_local_part_store(&launch_config)?;

    crate::commands::system::apply::execute_apply(crate::commands::system::apply::ApplyArgs {
        targets: args.plan_targets,
        fresh: false,
        deps: None,
        rdeps: None,
        match_policies: Vec::new(),
        depth: None,
        force: args.force,
        dry_run: args.dry_run,
        config: &launch_config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store: &part_store,
    })
    .await
}

fn ensure_target_skeleton(root_dir: &Path) -> std::io::Result<()> {
    for sub in [
        "var/lib/wright",
        "var/lib/wright/parts",
        "var/lib/wright/staging",
        "var/lib/wright/pack-staging",
        "var/lib/wright/lock",
        "var/log/wright",
        "etc/wright",
    ] {
        std::fs::create_dir_all(root_dir.join(sub))?;
    }
    Ok(())
}

fn part_store_for_target(root_dir: &Path) -> LocalPartStore {
    let mut store = LocalPartStore::new();
    store.add_search_dir(root_dir.join("var/lib/wright/parts"));
    store
}

fn resolve_profile(profile: &str, pack_dirs: &[PathBuf]) -> Result<PathBuf> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for dir in pack_dirs {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !fname.ends_with(pack::PACK_FILE_SUFFIX) {
                continue;
            }
            let manifest = match pack::read_manifest(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if manifest.pack.name == profile {
                candidates.push((manifest.pack.version, path));
            }
        }
    }
    if candidates.is_empty() {
        anyhow::bail!(
            "profile '{}' not found in {}",
            profile,
            pack_dirs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(candidates.pop().unwrap().1)
}

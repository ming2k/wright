use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::operations::apply::{execute_apply, ApplyRequest};
use crate::part::group::{self, GroupManifest};
use crate::part::store::LocalPartStore;

pub struct LaunchRequest {
    pub group: Option<PathBuf>,
    pub plans: Option<PathBuf>,
    pub plan_targets: Vec<String>,
    pub dry_run: bool,
    pub force: bool,
}

pub async fn execute_launch(
    request: LaunchRequest,
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

    // Isolate build outputs under the target root so the host is not polluted.
    let mut launch_config = config.clone();
    redirect_build_paths_for_target(&mut launch_config, root_dir);

    if let Some(group_path) = request.group.clone() {
        let manifest = group::read_manifest(&group_path)
            .with_context(|| format!("read group {}", group_path.display()))?;
        return launch_from_group(
            manifest,
            &request,
            &launch_config,
            db_path,
            root_dir,
            verbose,
            quiet,
        )
        .await;
    }

    if let Some(plans_dir) = request.plans.clone() {
        return launch_from_plans(
            &request,
            plans_dir,
            &launch_config,
            db_path,
            root_dir,
            verbose,
            quiet,
        )
        .await;
    }

    if !request.plan_targets.is_empty() {
        return launch_from_plans(
            &request,
            config.general.plans_dir.clone(),
            &launch_config,
            db_path,
            root_dir,
            verbose,
            quiet,
        )
        .await;
    }

    anyhow::bail!("wright launch needs --group or targets to launch; nothing to do")
}

async fn launch_from_group(
    manifest: GroupManifest,
    request: &LaunchRequest,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    println!(
        "launching group {} {} into {}",
        manifest.group.name,
        manifest.group.version,
        root_dir.display()
    );
    info!(
        "group carries {} plan(s), {} assumption(s){}",
        manifest.group.plans.len(),
        manifest.assumes.len(),
        if manifest.config.is_some() {
            ", config block"
        } else {
            ""
        }
    );

    if request.dry_run {
        println!();
        println!(
            "[dry-run] would build and install {} plan(s):",
            manifest.group.plans.len()
        );
        for p in &manifest.group.plans {
            println!("  {}", p);
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

    // Copy plans and groups into the target so the installed system can
    // self-maintain without referring back to the host tree.
    let target_plans_dir = root_dir.join("var/lib/wright/plans");
    let target_groups_dir = root_dir.join("var/lib/wright/groups");
    sync_plans_to_target(&config.general.plans_dir, &target_plans_dir)
        .context("sync host plans into target root")?;
    for extra_dir in &config.general.extra_plans_dirs {
        sync_plans_to_target(extra_dir, &target_plans_dir)
            .context("sync extra plans into target root")?;
    }
    if let Some(group_path) = request.group.as_ref() {
        sync_groups_to_target(&[group_path.clone()], &target_groups_dir)
            .context("sync group manifest into target root")?;
    }
    write_target_wright_toml(root_dir, config).context("write target wright.toml")?;

    // Pre-register external assumptions.
    let db = InstalledDb::open(db_path)
        .await
        .context("failed to open target database")?;
    for assume in &manifest.assumes {
        db.assume_part(&assume.name, &assume.version)
            .await
            .with_context(|| format!("failed to assume {}", assume.name))?;
    }
    drop(db);

    // Apply config (hostname, timezone, locale, services) after install.
    let part_store = setup_part_store(config)?;

    build_and_apply(
        ApplyRequest {
            targets: manifest.group.plans.clone(),
            deps: None,
            rdeps: None,
            match_policies: Vec::new(),
            depth: None,
            force: request.force,
            config,
            db_path,
            root_dir,
            verbose,
            quiet,
            part_store: &part_store,
        },
        false,
    )
    .await?;

    // Apply group config if present.
    if let Some(cfg) = manifest.config.as_ref() {
        apply_group_config(root_dir, cfg).await?;
    }

    println!("launched {} -> {}", manifest.group.name, root_dir.display());
    Ok(())
}

async fn launch_from_plans(
    request: &LaunchRequest,
    plans_dir: PathBuf,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    if request.plan_targets.is_empty() {
        anyhow::bail!("--plans needs one or more plan or group names to launch");
    }

    let mut launch_config = config.clone();
    if plans_dir != launch_config.general.plans_dir {
        launch_config
            .general
            .extra_plans_dirs
            .push(plans_dir.clone());
    }

    let groups_dirs: Vec<PathBuf> = vec![config.general.groups_dir.clone()];

    let (targets, group_assumes, group_config) =
        group::expand_group_references(request.plan_targets.clone(), &groups_dirs)?;

    if targets.is_empty() {
        anyhow::bail!("no plans to build after expanding groups");
    }

    // Copy plans and groups into the target so the installed system can
    // self-maintain without referring back to the host tree.
    let target_plans_dir = root_dir.join("var/lib/wright/plans");
    let target_groups_dir = root_dir.join("var/lib/wright/groups");
    sync_plans_to_target(&plans_dir, &target_plans_dir)
        .context("sync source plans into target root")?;
    sync_plans_to_target(&config.general.plans_dir, &target_plans_dir)
        .context("sync host plans into target root")?;
    for extra_dir in &config.general.extra_plans_dirs {
        sync_plans_to_target(extra_dir, &target_plans_dir)
            .context("sync extra plans into target root")?;
    }

    // Collect group files that were referenced and sync them too.
    let mut referenced_groups: Vec<PathBuf> = Vec::new();
    for target in &request.plan_targets {
        if let Some(group_name) = target.strip_prefix('@') {
            if let Some(group_path) = group::find_group_manifest(&groups_dirs, group_name) {
                referenced_groups.push(group_path);
            }
        }
    }
    if !referenced_groups.is_empty() {
        sync_groups_to_target(&referenced_groups, &target_groups_dir)
            .context("sync referenced groups into target root")?;
    }
    write_target_wright_toml(root_dir, config).context("write target wright.toml")?;

    let part_store = setup_part_store(config)?;

    // Pre-register any assumptions collected from groups.
    if !group_assumes.is_empty() {
        let db = InstalledDb::open(db_path)
            .await
            .context("failed to open target database")?;
        for assume in &group_assumes {
            db.assume_part(&assume.name, &assume.version)
                .await
                .with_context(|| format!("failed to assume {}", assume.name))?;
        }
        drop(db);
    }

    build_and_apply(
        ApplyRequest {
            targets,
            deps: None,
            rdeps: None,
            match_policies: Vec::new(),
            depth: None,
            force: request.force,
            config: &launch_config,
            db_path,
            root_dir,
            verbose,
            quiet,
            part_store: &part_store,
        },
        request.dry_run,
    )
    .await?;

    if let Some(cfg) = group_config.as_ref() {
        apply_group_config(root_dir, cfg).await?;
    }

    println!("launched plans -> {}", root_dir.display());
    Ok(())
}

fn redirect_build_paths_for_target(config: &mut GlobalConfig, root_dir: &Path) {
    // When targeting an alternate root, build and package outputs should
    // live under that root so the host system is not polluted.
    config.general.parts_dir = root_dir.join("var/lib/wright/parts");
    config.build.build_dir = root_dir.join("var/tmp/wright/workshop");
}

fn setup_part_store(config: &GlobalConfig) -> Result<LocalPartStore> {
    crate::planning::setup_part_store(config).map_err(Into::into)
}

async fn build_and_apply(request: ApplyRequest<'_>, dry_run: bool) -> Result<()> {
    if dry_run {
        println!("Apply plan (dry-run):");
        println!("  targets: {}", request.targets.join(", "));
        return Ok(());
    }

    execute_apply(request).await
}

fn ensure_target_skeleton(root_dir: &Path) -> std::io::Result<()> {
    for sub in [
        "var/lib/wright",
        "var/lib/wright/parts",
        "var/lib/wright/staging",
        "var/lib/wright/lock",
        "var/lib/wright/plans",
        "var/lib/wright/groups",
        "var/log/wright",
        "etc/wright",
    ] {
        std::fs::create_dir_all(root_dir.join(sub))?;
    }
    Ok(())
}

/// Synchronise plan directories from a source tree into the target root.
/// Only syncs directories that contain a `plan.toml` file.
/// Compares mtime + size to avoid unnecessary copies.
fn sync_plans_to_target(source_dir: &Path, target_plans_dir: &Path) -> Result<()> {
    if !source_dir.exists() {
        return Ok(());
    }

    let mut total_stats = SyncStats::default();
    let mut plan_count = 0;

    for entry in std::fs::read_dir(source_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let plan_toml = path.join("plan.toml");
        if !plan_toml.is_file() {
            continue;
        }
        let plan_name = path.file_name().unwrap_or_default();
        let target_plan_dir = target_plans_dir.join(&plan_name);
        let stats = sync_dir_with_stats(&path, &target_plan_dir)
            .with_context(|| format!("sync plan {} to target", plan_name.to_string_lossy()))?;
        total_stats.copied += stats.copied;
        total_stats.skipped += stats.skipped;
        total_stats.removed += stats.removed;
        plan_count += 1;

        if stats.copied == 0 && stats.removed == 0 {
            debug!("Plan {} up-to-date", plan_name.to_string_lossy());
        } else {
            debug!(
                "Plan {} synced ({} copied, {} skipped, {} removed)",
                plan_name.to_string_lossy(),
                stats.copied,
                stats.skipped,
                stats.removed
            );
        }
    }

    if plan_count > 0 {
        info!(
            "Synced {} plan(s): {} copied, {} skipped, {} removed",
            plan_count, total_stats.copied, total_stats.skipped, total_stats.removed
        );
    }
    Ok(())
}

/// Synchronise group manifest files into the target root.
/// Compares mtime + size to avoid unnecessary copies.
fn sync_groups_to_target(source_groups: &[PathBuf], target_groups_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(target_groups_dir)?;
    let mut synced = 0;
    let mut up_to_date = 0;
    for source in source_groups {
        let file_name = source.file_name().unwrap_or_default();
        let target = target_groups_dir.join(file_name);
        if should_copy_file(source, &target) {
            std::fs::copy(source, &target)
                .with_context(|| format!("sync group {} to target", file_name.to_string_lossy()))?;
            debug!("Group {} synced", file_name.to_string_lossy());
            synced += 1;
        } else {
            debug!("Group {} up-to-date", file_name.to_string_lossy());
            up_to_date += 1;
        }
    }
    if synced > 0 || up_to_date > 0 {
        info!(
            "Synced {} group(s): {} updated, {} up-to-date",
            synced + up_to_date,
            synced,
            up_to_date
        );
    }
    Ok(())
}

#[derive(Default)]
struct SyncStats {
    copied: usize,
    skipped: usize,
    removed: usize,
}

/// Recursively synchronise `src` into `dst`.
///
/// * Files are copied only when the source differs from the destination
///   (by size or mtime).
/// * Entries that exist in `dst` but not in `src` are removed so the
///   target stays an accurate mirror of the host.
///
/// Returns a [`SyncStats`] describing how many files were copied, skipped,
/// and removed.
fn sync_dir_with_stats(src: &Path, dst: &Path) -> std::io::Result<SyncStats> {
    std::fs::create_dir_all(dst)?;
    let mut stats = SyncStats::default();

    // 1. Copy / update everything that exists in src.
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let name = entry.file_name();
        let dst_path = dst.join(&name);

        if src_path.is_dir() {
            let sub = sync_dir_with_stats(&src_path, &dst_path)?;
            stats.copied += sub.copied;
            stats.skipped += sub.skipped;
            stats.removed += sub.removed;
        } else if should_copy_file(&src_path, &dst_path) {
            std::fs::copy(&src_path, &dst_path)?;
            stats.copied += 1;
        } else {
            stats.skipped += 1;
        }
    }

    // 2. Remove anything in dst that no longer exists in src.
    if let Ok(entries) = std::fs::read_dir(dst) {
        for entry in entries {
            let entry = entry?;
            let dst_path = entry.path();
            let name = entry.file_name();
            let src_path = src.join(&name);
            if !src_path.exists() {
                if dst_path.is_dir() {
                    std::fs::remove_dir_all(&dst_path)?;
                } else {
                    std::fs::remove_file(&dst_path)?;
                }
                stats.removed += 1;
            }
        }
    }

    Ok(stats)
}

/// Return `true` when `dst` does not exist or differs from `src`.
fn should_copy_file(src: &Path, dst: &Path) -> bool {
    if !dst.is_file() {
        return true;
    }
    let src_meta = match std::fs::metadata(src) {
        Ok(m) => m,
        Err(_) => return true,
    };
    let dst_meta = match std::fs::metadata(dst) {
        Ok(m) => m,
        Err(_) => return true,
    };
    src_meta.len() != dst_meta.len() || file_mtime(&src_meta) != file_mtime(&dst_meta)
}

#[cfg(unix)]
fn file_mtime(meta: &std::fs::Metadata) -> std::time::SystemTime {
    use std::os::unix::fs::MetadataExt;
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(meta.mtime() as u64)
}

#[cfg(windows)]
fn file_mtime(meta: &std::fs::Metadata) -> std::time::SystemTime {
    meta.modified().unwrap_or(std::time::UNIX_EPOCH)
}

/// Write a minimal wright.toml into the target root so the installed
/// system can operate independently.
fn write_target_wright_toml(root_dir: &Path, config: &GlobalConfig) -> Result<()> {
    let target_config = root_dir.join("etc/wright/wright.toml");
    std::fs::create_dir_all(target_config.parent().unwrap())?;

    let content = format!(
        r#"[general]
arch = "{}"
plans_dir = "/var/lib/wright/plans"
groups_dir = "/var/lib/wright/groups"
parts_dir = "/var/lib/wright/parts"
source_dir = "/var/lib/wright/sources"
db_path = "/var/lib/wright/wright.db"
logs_dir = "/var/log/wright"
executors_dir = "/etc/wright/executors"

[build]
build_dir = "/var/tmp/wright/workshop"
default_isolation = "{}"
"#,
        config.general.arch, config.build.default_isolation,
    );
    std::fs::write(&target_config, content)
        .with_context(|| format!("write {}", target_config.display()))?;
    Ok(())
}

async fn apply_group_config(root_dir: &Path, cfg: &group::GroupConfig) -> Result<()> {
    if let Some(ref hostname) = cfg.hostname {
        let path = root_dir.join("etc/hostname");
        std::fs::write(&path, format!("{}\n", hostname))
            .map_err(|e| anyhow::anyhow!("write hostname: {}", e))?;
    }
    if let Some(ref tz) = cfg.timezone {
        let target = format!("../usr/share/zoneinfo/{}", tz);
        let link = root_dir.join("etc/localtime");
        let _ = std::fs::remove_file(&link);
        if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
            warn!("failed to symlink {} -> {}: {}", link.display(), target, e);
        }
    }
    if let Some(ref locale) = cfg.locale {
        let path = root_dir.join("etc/locale.conf");
        std::fs::write(&path, format!("LANG={}\n", locale))
            .map_err(|e| anyhow::anyhow!("write locale: {}", e))?;
    }
    if !cfg.services.is_empty() {
        let svc_root = root_dir.join("var/service");
        std::fs::create_dir_all(&svc_root)
            .map_err(|e| anyhow::anyhow!("mkdir var/service: {}", e))?;
        for service in &cfg.services {
            let target = format!("/etc/sv/{}", service);
            let link = svc_root.join(service);
            if link.exists() {
                continue;
            }
            if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
                warn!(
                    "failed to enable runit service {}: {} -> {}: {}",
                    service,
                    link.display(),
                    target,
                    e
                );
            }
        }
    }
    Ok(())
}

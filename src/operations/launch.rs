use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};
use tracing::{debug, info, warn};

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::operations::install::{InstallRequest, execute_install};
use crate::part::folio::{self, FolioManifest};
use crate::part::store::LocalPartStore;
use crate::resolve::DepDomain;

pub struct LaunchRequest {
    pub folio: Option<PathBuf>,
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
        return Err(WrightError::ForgeError(
            "wright launch refuses to fill `/`; pass --root <PATH> pointing at a mounted target root".into()
        ));
    }
    ensure_target_skeleton(root_dir).map_err(|e| {
        WrightError::ForgeError(format!(
            "failed to prepare target root {}: {}",
            root_dir.display(),
            e
        ))
    })?;

    // Isolate build outputs under the target root so the host is not polluted.
    let mut launch_config = config.clone();
    redirect_build_paths_for_target(&mut launch_config, root_dir);

    if let Some(folio_path) = request.folio.clone() {
        let manifest = folio::read_manifest(&folio_path).map_err(|e| {
            WrightError::ForgeError(format!("read folio {}: {}", folio_path.display(), e))
        })?;
        return launch_from_folio(
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

    Err(WrightError::ForgeError(
        "wright launch needs --folio or targets to launch; nothing to do".into(),
    ))
}

async fn launch_from_folio(
    manifest: FolioManifest,
    request: &LaunchRequest,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    println!(
        "launching folio {} {} into {}",
        manifest.folio.name,
        manifest.folio.version,
        root_dir.display()
    );
    info!(
        event = "launch.folio_loaded",
        plan_count = manifest.folio.plans.len(),
        assumption_count = manifest.provides.len(),
        has_config = manifest.config.is_some(),
        "Folio loaded"
    );

    if request.dry_run {
        println!();
        println!(
            "[dry-run] would forge and deploy {} plan(s):",
            manifest.folio.plans.len()
        );
        for p in &manifest.folio.plans {
            println!("  {}", p);
        }
        if !manifest.provides.is_empty() {
            println!();
            println!(
                "[dry-run] would assume {} external(s):",
                manifest.provides.len()
            );
            for a in &manifest.provides {
                println!("  {} {}", a.name, a.version);
            }
        }
        return Ok(());
    }

    // Copy plans and folios into the target so the deployed system can
    // self-maintain without referring back to the host tree.
    let target_plans_dir = root_dir.join("var/lib/wright/plans");
    let target_folios_dir = root_dir.join("var/lib/wright/folios");
    sync_plans_to_target(&config.general.plans_dir, &target_plans_dir)
        .map_err(|e| WrightError::ForgeError(format!("sync host plans into target root: {}", e)))?;
    for extra_dir in &config.general.extra_plans_dirs {
        sync_plans_to_target(extra_dir, &target_plans_dir).map_err(|e| {
            WrightError::ForgeError(format!("sync extra plans into target root: {}", e))
        })?;
    }
    if let Some(folio_path) = request.folio.as_ref() {
        sync_folios_to_target(&[folio_path.clone()], &target_folios_dir).map_err(|e| {
            WrightError::ForgeError(format!("sync folio manifest into target root: {}", e))
        })?;
    }
    write_target_wright_toml(root_dir, config)
        .map_err(|e| WrightError::ForgeError(format!("write target wright.toml: {}", e)))?;

    // Pre-register external assumptions.
    let db = InstalledDb::open(db_path).await.map_err(|e| {
        WrightError::DatabaseError(format!("failed to open target database: {}", e))
    })?;
    for assume in &manifest.provides {
        db.provide_part(&assume.name, &assume.version)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to assume {}: {}", assume.name, e))
            })?;
    }
    drop(db);

    // Apply config (hostname, timezone, locale, services) after install.
    let part_store = setup_part_store(config)?;

    build_and_apply(
        InstallRequest {
            forge_opts: None,
            targets: manifest.folio.plans.clone(),
            dep_domain: DepDomain::empty(),
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

    // Apply folio config if present.
    if let Some(cfg) = manifest.config.as_ref() {
        apply_folio_config(root_dir, cfg).await?;
    }

    println!("launched {} -> {}", manifest.folio.name, root_dir.display());
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
        return Err(WrightError::ForgeError(
            "--plans needs one or more plan or folio names to launch".into(),
        ));
    }

    let mut launch_config = config.clone();
    if plans_dir != launch_config.general.plans_dir {
        launch_config
            .general
            .extra_plans_dirs
            .push(plans_dir.clone());
    }

    let folios_dirs: Vec<PathBuf> = vec![config.general.folios_dir.clone()];

    let (targets, folio_provides, folio_config) =
        folio::expand_folio_references(request.plan_targets.clone(), &folios_dirs)?;

    if targets.is_empty() {
        return Err(WrightError::ForgeError(
            "no plans to forge after expanding folios".into(),
        ));
    }

    // Copy plans and folios into the target so the deployed system can
    // self-maintain without referring back to the host tree.
    let target_plans_dir = root_dir.join("var/lib/wright/plans");
    let target_folios_dir = root_dir.join("var/lib/wright/folios");
    sync_plans_to_target(&plans_dir, &target_plans_dir).map_err(|e| {
        WrightError::ForgeError(format!("sync source plans into target root: {}", e))
    })?;
    sync_plans_to_target(&config.general.plans_dir, &target_plans_dir)
        .map_err(|e| WrightError::ForgeError(format!("sync host plans into target root: {}", e)))?;
    for extra_dir in &config.general.extra_plans_dirs {
        sync_plans_to_target(extra_dir, &target_plans_dir).map_err(|e| {
            WrightError::ForgeError(format!("sync extra plans into target root: {}", e))
        })?;
    }

    // Collect folio files that were referenced and sync them too.
    let mut referenced_folios: Vec<PathBuf> = Vec::new();
    for target in &request.plan_targets {
        if let Some(folio_name) = target.strip_prefix('@')
            && let Some(folio_path) = folio::find_folio_manifest(&folios_dirs, folio_name)
        {
            referenced_folios.push(folio_path);
        }
    }
    if !referenced_folios.is_empty() {
        sync_folios_to_target(&referenced_folios, &target_folios_dir).map_err(|e| {
            WrightError::ForgeError(format!("sync referenced folios into target root: {}", e))
        })?;
    }
    write_target_wright_toml(root_dir, config)
        .map_err(|e| WrightError::ForgeError(format!("write target wright.toml: {}", e)))?;

    let part_store = setup_part_store(config)?;

    // Pre-register any assumptions collected from folios.
    if !folio_provides.is_empty() {
        let db = InstalledDb::open(db_path).await.map_err(|e| {
            WrightError::DatabaseError(format!("failed to open target database: {}", e))
        })?;
        for provide in &folio_provides {
            db.provide_part(&provide.name, &provide.version)
                .await
                .map_err(|e| {
                    WrightError::DatabaseError(format!("failed to assume {}: {}", provide.name, e))
                })?;
        }
        drop(db);
    }

    build_and_apply(
        InstallRequest {
            forge_opts: None,
            targets,
            dep_domain: DepDomain::empty(),
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

    if let Some(cfg) = folio_config.as_ref() {
        apply_folio_config(root_dir, cfg).await?;
    }

    println!("launched plans -> {}", root_dir.display());
    Ok(())
}

fn redirect_build_paths_for_target(config: &mut GlobalConfig, root_dir: &Path) {
    // When targeting an alternate root, forge and seal outputs should
    // live under that root so the host system is not polluted.
    config.general.parts_dir = root_dir.join("var/lib/wright/parts");
    config.build.forge_dir = root_dir.join("var/tmp/wright/workshop");
}

fn setup_part_store(config: &GlobalConfig) -> Result<LocalPartStore> {
    crate::resolve::setup_part_store(config)
}

async fn build_and_apply(request: InstallRequest<'_>, dry_run: bool) -> Result<()> {
    if dry_run {
        println!("Apply plan (dry-run):");
        println!("  targets: {}", request.targets.join(", "));
        return Ok(());
    }

    execute_install(request).await
}

fn ensure_target_skeleton(root_dir: &Path) -> std::io::Result<()> {
    for sub in [
        "var/lib/wright",
        "var/lib/wright/parts",
        "var/lib/wright/staging",
        "var/lib/wright/lock",
        "var/lib/wright/plans",
        "var/lib/wright/folios",
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
        let target_plan_dir = target_plans_dir.join(plan_name);
        let stats = sync_dir_with_stats(&path, &target_plan_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "sync plan {} to target: {}",
                plan_name.to_string_lossy(),
                e
            ))
        })?;
        total_stats.copied += stats.copied;
        total_stats.skipped += stats.skipped;
        total_stats.removed += stats.removed;
        plan_count += 1;

        if stats.copied == 0 && stats.removed == 0 {
            debug!(
                event = "launch.plan_uptodate",
                plan_name = %plan_name.to_string_lossy(),
                "Plan up-to-date"
            );
        } else {
            debug!(
                event = "launch.plan_synced",
                plan_name = %plan_name.to_string_lossy(),
                copied = stats.copied,
                skipped = stats.skipped,
                removed = stats.removed,
                "Plan synced"
            );
        }
    }

    if plan_count > 0 {
        info!(
            event = "launch.plans_synced",
            plan_count,
            copied = total_stats.copied,
            skipped = total_stats.skipped,
            removed = total_stats.removed,
            "Plans synced"
        );
    }
    Ok(())
}

/// Synchronise folio manifest files into the target root.
/// Compares mtime + size to avoid unnecessary copies.
fn sync_folios_to_target(source_folios: &[PathBuf], target_folios_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(target_folios_dir)?;
    let mut synced = 0;
    let mut up_to_date = 0;
    for source in source_folios {
        let file_name = source.file_name().unwrap_or_default();
        let target = target_folios_dir.join(file_name);
        if should_copy_file(source, &target) {
            std::fs::copy(source, &target).map_err(|e| {
                WrightError::ForgeError(format!(
                    "sync folio {} to target: {}",
                    file_name.to_string_lossy(),
                    e
                ))
            })?;
            debug!(
                event = "launch.folio_synced",
                folio_name = %file_name.to_string_lossy(),
                "Folio synced"
            );
            synced += 1;
        } else {
            debug!(
                event = "launch.folio_uptodate",
                folio_name = %file_name.to_string_lossy(),
                "Folio up-to-date"
            );
            up_to_date += 1;
        }
    }
    if synced > 0 || up_to_date > 0 {
        info!(
            event = "launch.folios_synced",
            total = synced + up_to_date,
            updated = synced,
            up_to_date,
            "Folios synced"
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

/// Write a minimal wright.toml into the target root so the deployed
/// system can operate independently.
fn write_target_wright_toml(root_dir: &Path, config: &GlobalConfig) -> Result<()> {
    let target_config = root_dir.join("etc/wright/wright.toml");
    std::fs::create_dir_all(target_config.parent().unwrap())?;

    let content = format!(
        r#"[general]
arch = "{}"
plans_dir = "/var/lib/wright/plans"
folios_dir = "/var/lib/wright/folios"
parts_dir = "/var/lib/wright/parts"
source_dir = "/var/lib/wright/sources"
db_path = "/var/lib/wright/wright.db"
logs_dir = "/var/log/wright"
executors_dir = "/etc/wright/executors"

[build]
forge_dir = "/var/tmp/wright/workshop"
default_isolation = "{}"
"#,
        config.general.arch, config.build.default_isolation,
    );
    std::fs::write(&target_config, content).map_err(WrightError::IoError)?;
    Ok(())
}

async fn apply_folio_config(root_dir: &Path, cfg: &folio::FolioConfig) -> Result<()> {
    if let Some(ref hostname) = cfg.hostname {
        let path = root_dir.join("etc/hostname");
        std::fs::write(&path, format!("{}\n", hostname)).map_err(WrightError::IoError)?;
    }
    if let Some(ref tz) = cfg.timezone {
        let target = format!("../usr/share/zoneinfo/{}", tz);
        let link = root_dir.join("etc/localtime");
        let _ = std::fs::remove_file(&link);
        if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
            warn!(
                event = "launch.symlink_failed",
                link = %link.display(),
                target = %target,
                error = %e,
                "Failed to symlink timezone"
            );
        }
    }
    if let Some(ref locale) = cfg.locale {
        let path = root_dir.join("etc/locale.conf");
        std::fs::write(&path, format!("LANG={}\n", locale)).map_err(WrightError::IoError)?;
    }
    if !cfg.services.is_empty() {
        let svc_root = root_dir.join("var/service");
        std::fs::create_dir_all(&svc_root).map_err(WrightError::IoError)?;
        for service in &cfg.services {
            let target = format!("/etc/sv/{}", service);
            let link = svc_root.join(service);
            if link.exists() {
                continue;
            }
            if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
                warn!(
                    event = "launch.service_enable_failed",
                    service,
                    link = %link.display(),
                    target = %target,
                    error = %e,
                    "Failed to enable runit service"
                );
            }
        }
    }
    Ok(())
}

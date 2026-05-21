//! `wright launch` — fill an empty target root with a coherent Wright system.
//!
//! Launch is the bootstrap pipeline: refuse `/`, resolve folio references,
//! sync plan and folio sources into the target, redirect all build artefact
//! paths into the target, register external assumptions, drive the install
//! pipeline wave by wave, and run post-launch hooks.
//!
//! Re-running launch against the same root **converges drift** — unchanged
//! plans are skipped, missing ones are built, changed ones are rebuilt.

use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::operations::install::{InstallRequest, execute_install};
use crate::part::folio::{self, Expansion, FolioManifest, FolioProvide, Hook, HookStage};
use crate::resolve::DepDomain;

/// A single user request to launch a folio or set of plans into `root`.
pub struct LaunchRequest {
    pub source: LaunchSource,
    pub dry_run: bool,
    pub force: bool,
}

/// Where the launch's plan list comes from.
pub enum LaunchSource {
    /// Direct path to a single `folio.toml`.
    Folio(PathBuf),
    /// Mix of plan names and `@folio` references, with optional
    /// overrides for the plans and folios search roots.
    Targets {
        plans_dir: Option<PathBuf>,
        folios_dir: Option<PathBuf>,
        targets: Vec<String>,
    },
}

/// Resolved launch plan after folio expansion.  Contains everything the
/// later stages need without further filesystem lookup.
struct LaunchPlan {
    label: String,
    targets: Vec<String>,
    provides: Vec<FolioProvide>,
    hooks: Vec<Hook>,
    /// Folio manifest files to mirror into the target root.
    folio_files: Vec<PathBuf>,
    /// Plan source directories to mirror into the target root.
    plans_dirs: Vec<PathBuf>,
}

// ── Entry point ────────────────────────────────────────────────────────

pub async fn execute_launch(
    request: LaunchRequest,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    if root_dir == Path::new("/") {
        return Err(forge_err(
            "wright launch refuses to fill `/`; pass --root <PATH> pointing at a mounted target root",
        ));
    }

    let plan = resolve_launch_plan(&request.source, config)?;

    if request.dry_run {
        print_dry_run(&plan, root_dir);
        return Ok(());
    }

    ensure_target_skeleton(root_dir)
        .map_err(|e| forge_err(format!("prepare target root {}: {}", root_dir.display(), e)))?;

    let launch_config = redirect_for_target(config, root_dir);

    sync_sources(&plan, &launch_config, root_dir)?;
    write_target_wright_toml(root_dir, &launch_config)?;
    register_provides(db_path, &plan.provides).await?;

    let part_store = crate::resolve::setup_part_store(&launch_config)?;
    execute_install(InstallRequest {
        build_opts: None,
        targets: plan.targets.clone(),
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
        run_hooks: false,
    })
    .await?;

    run_hooks(root_dir, &plan.hooks).await?;

    println!("launched {} -> {}", plan.label, root_dir.display());
    Ok(())
}

// ── Plan resolution ────────────────────────────────────────────────────

fn resolve_launch_plan(source: &LaunchSource, config: &GlobalConfig) -> Result<LaunchPlan> {
    match source {
        LaunchSource::Folio(path) => {
            let manifest = FolioManifest::load(path)?;
            let plans_dirs = host_plans_dirs(config, None);
            Ok(LaunchPlan {
                label: format!("folio {} {}", manifest.folio.name, manifest.folio.version),
                targets: manifest.folio.plans,
                provides: manifest.provides,
                hooks: manifest.hooks,
                folio_files: vec![path.clone()],
                plans_dirs,
            })
        }
        LaunchSource::Targets {
            plans_dir,
            folios_dir,
            targets,
        } => {
            if targets.is_empty() {
                return Err(forge_err(
                    "wright launch needs --folio or one or more targets; nothing to do",
                ));
            }
            let plans_dirs = host_plans_dirs(config, plans_dir.as_deref());
            let folio_dirs = folio_search_dirs(config, folios_dir.as_deref());
            let expansion: Expansion = folio::expand(targets, &folio_dirs)?;
            if expansion.plans.is_empty() {
                return Err(forge_err("no plans to forge after expanding folios"));
            }
            Ok(LaunchPlan {
                label: "plans".into(),
                targets: expansion.plans,
                provides: expansion.provides,
                hooks: expansion.hooks,
                folio_files: expansion.referenced,
                plans_dirs,
            })
        }
    }
}

/// Plan source directories on the host, in priority order, deduplicated.
fn host_plans_dirs(config: &GlobalConfig, override_: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(o) = override_ {
        dirs.push(o.to_path_buf());
    }
    dirs.push(config.general.plans_dir.clone());
    dirs.extend(config.general.extra_plans_dirs.iter().cloned());
    dedup(dirs)
}

/// Folio search directories.  Folios are peers of plans, not nested inside
/// them: only `--folios <DIR>` (if given) and the configured `folios_dir`
/// are consulted.
fn folio_search_dirs(config: &GlobalConfig, override_: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(o) = override_ {
        dirs.push(o.to_path_buf());
    }
    dirs.push(config.general.folios_dir.clone());
    dedup(dirs)
}

fn dedup(v: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    v.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

// ── Dry run ────────────────────────────────────────────────────────────

fn print_dry_run(plan: &LaunchPlan, root_dir: &Path) {
    println!("[dry-run] {} -> {}", plan.label, root_dir.display());
    println!("[dry-run] would forge and deploy {} plan(s):", plan.targets.len());
    for t in &plan.targets {
        println!("  {t}");
    }
    if !plan.provides.is_empty() {
        println!("[dry-run] would assume {} external(s):", plan.provides.len());
        for p in &plan.provides {
            println!("  {} {}", p.name, p.version);
        }
    }
    if !plan.hooks.is_empty() {
        println!("[dry-run] would run {} post-launch hook(s)", plan.hooks.len());
    }
}

// ── Side effects ───────────────────────────────────────────────────────

/// Create the directory layout the target needs before any artefact lands.
fn ensure_target_skeleton(root_dir: &Path) -> std::io::Result<()> {
    for sub in [
        "var/lib/wright",
        "var/lib/wright/parts",
        "var/lib/wright/store",
        "var/lib/wright/staging",
        "var/lib/wright/lock",
        "var/lib/wright/plans",
        "var/lib/wright/folios",
        "var/lib/wright/sources",
        "var/log/wright",
        "var/tmp/wright",
        "etc/wright",
    ] {
        std::fs::create_dir_all(root_dir.join(sub))?;
    }
    Ok(())
}

/// Build a config that points every artefact path at the target root, so a
/// launch never pollutes the host's part store, source cache, or logs.
///
/// `plans_dir`, `folios_dir`, and `executors_dir` stay on the host: their
/// host copies are the source of truth for the current launch, and target
/// copies are populated by [`sync_sources`].
fn redirect_for_target(config: &GlobalConfig, root_dir: &Path) -> GlobalConfig {
    let mut out = config.clone();
    out.general.parts_dir = root_dir.join("var/lib/wright/parts");
    out.general.store_dir = root_dir.join("var/lib/wright/store");
    out.general.source_dir = root_dir.join("var/lib/wright/sources");
    out.general.logs_dir = root_dir.join("var/log/wright");
    out.build.forge_dir = root_dir.join("var/tmp/wright/workshop");
    out
}

/// Mirror host plan directories and folio files into the target so the
/// deployed system can self-maintain without referring back to the host.
fn sync_sources(plan: &LaunchPlan, config: &GlobalConfig, root_dir: &Path) -> Result<()> {
    let target_plans = root_dir.join("var/lib/wright/plans");
    let target_folios = root_dir.join("var/lib/wright/folios");

    for dir in &plan.plans_dirs {
        sync_plans_dir(dir, &target_plans)?;
    }
    // The host's `extra_plans_dirs` may sit outside the plans search dirs the
    // launch used; mirror them too so the deployed system sees the same plan
    // set as the host.
    for dir in &config.general.extra_plans_dirs {
        if !plan.plans_dirs.contains(dir) {
            sync_plans_dir(dir, &target_plans)?;
        }
    }
    sync_folios(&plan.folio_files, &target_folios)?;
    Ok(())
}

/// Mirror every plan directory under `source` (any subdir containing a
/// `plan.toml`) into `target_plans`.
fn sync_plans_dir(source: &Path, target_plans: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    let mut totals = SyncStats::default();
    let mut count = 0;
    for entry in std::fs::read_dir(source).map_err(forge_io)? {
        let entry = entry.map_err(forge_io)?;
        let path = entry.path();
        if !path.is_dir() || !path.join("plan.toml").is_file() {
            continue;
        }
        let name = entry.file_name();
        let dest = target_plans.join(&name);
        let stats = mirror_dir(&path, &dest).map_err(|e| {
            forge_err(format!("sync plan {}: {}", name.to_string_lossy(), e))
        })?;
        debug!(
            event = "launch.plan_synced",
            plan = %name.to_string_lossy(),
            copied = stats.copied,
            removed = stats.removed,
            "plan synced"
        );
        totals.add(&stats);
        count += 1;
    }
    if count > 0 {
        info!(
            event = "launch.plans_synced",
            plans = count,
            copied = totals.copied,
            removed = totals.removed,
            "plan sources synced"
        );
    }
    Ok(())
}

/// Copy each referenced folio file into the target.
fn sync_folios(sources: &[PathBuf], target: &Path) -> Result<()> {
    if sources.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(target).map_err(forge_io)?;
    let mut copied = 0;
    for src in sources {
        let name = src
            .file_name()
            .ok_or_else(|| forge_err(format!("folio path has no file name: {}", src.display())))?;
        let dst = target.join(name);
        if needs_copy(src, &dst) {
            std::fs::copy(src, &dst).map_err(forge_io)?;
            copied += 1;
        }
    }
    info!(event = "launch.folios_synced", total = sources.len(), copied, "folios synced");
    Ok(())
}

#[derive(Default)]
struct SyncStats {
    copied: usize,
    removed: usize,
}

impl SyncStats {
    fn add(&mut self, other: &Self) {
        self.copied += other.copied;
        self.removed += other.removed;
    }
}

/// Recursively make `dst` a mirror of `src`: copy changed files, prune
/// entries that no longer exist in `src`.
fn mirror_dir(src: &Path, dst: &Path) -> std::io::Result<SyncStats> {
    std::fs::create_dir_all(dst)?;
    let mut stats = SyncStats::default();

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let s = entry.path();
        let name = entry.file_name();
        let d = dst.join(&name);
        if s.is_dir() {
            let sub = mirror_dir(&s, &d)?;
            stats.copied += sub.copied;
            stats.removed += sub.removed;
        } else if needs_copy(&s, &d) {
            std::fs::copy(&s, &d)?;
            stats.copied += 1;
        }
    }

    for entry in std::fs::read_dir(dst)? {
        let entry = entry?;
        let d = entry.path();
        let name = entry.file_name();
        if !src.join(&name).exists() {
            if d.is_dir() {
                std::fs::remove_dir_all(&d)?;
            } else {
                std::fs::remove_file(&d)?;
            }
            stats.removed += 1;
        }
    }

    Ok(stats)
}

/// True when `dst` is absent or differs from `src` by size or mtime.
fn needs_copy(src: &Path, dst: &Path) -> bool {
    if !dst.is_file() {
        return true;
    }
    let Ok(s) = std::fs::metadata(src) else { return true };
    let Ok(d) = std::fs::metadata(dst) else { return true };
    s.len() != d.len() || mtime(&s) != mtime(&d)
}

#[cfg(unix)]
fn mtime(meta: &std::fs::Metadata) -> std::time::SystemTime {
    use std::os::unix::fs::MetadataExt;
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(meta.mtime() as u64)
}

#[cfg(windows)]
fn mtime(meta: &std::fs::Metadata) -> std::time::SystemTime {
    meta.modified().unwrap_or(std::time::UNIX_EPOCH)
}

/// Write the deployed system's `wright.toml`.  Every path is target-local
/// so a fresh boot into the target has a working config.
fn write_target_wright_toml(root_dir: &Path, config: &GlobalConfig) -> Result<()> {
    let path = root_dir.join("etc/wright/wright.toml");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(forge_io)?;
    }

    let content = format!(
        r#"# Generated by `wright launch`.
[general]
arch          = "{arch}"
plans_dir     = "/var/lib/wright/plans"
folios_dir    = "/var/lib/wright/folios"
parts_dir     = "/var/lib/wright/parts"
store_dir     = "/var/lib/wright/store"
source_dir    = "/var/lib/wright/sources"
db_path       = "/var/lib/wright/wright.db"
logs_dir      = "/var/log/wright"
executors_dir = "/etc/wright/executors"

[build]
forge_dir         = "/var/tmp/wright/workshop"
default_isolation = "{isolation}"
"#,
        arch = config.general.arch,
        isolation = config.build.default_isolation,
    );
    std::fs::write(&path, content).map_err(forge_io)?;
    Ok(())
}

async fn register_provides(db_path: &Path, provides: &[FolioProvide]) -> Result<()> {
    if provides.is_empty() {
        return Ok(());
    }
    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open target database: {e}")))?;
    for p in provides {
        db.provide_part(&p.name, &p.version)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("assume {}: {}", p.name, e)))?;
    }
    Ok(())
}

async fn run_hooks(root_dir: &Path, hooks: &[Hook]) -> Result<()> {
    for hook in hooks {
        // Stop launching hooks if the user cancelled during the install phase
        // or between hooks.
        if crate::isolation::reaper::is_cancelled() {
            return Err(forge_err("cancelled by user"));
        }

        match hook.stage {
            HookStage::PostLaunch => {}
        }

        let script = hook.script.clone();
        let root = root_dir.to_path_buf();
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .env("WRIGHT_ROOT", &root)
                .env("ROOT", &root)
                .output()
        })
        .await
        .map_err(|e| forge_err(format!("hook join: {e}")))?
        .map_err(|e| forge_err(format!("spawn hook: {e}")))?;

        if !output.status.success() {
            return Err(forge_err(format!(
                "post-launch hook failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        debug!(event = "launch.hook_ok", "hook ok");
    }
    Ok(())
}

// ── Errors ─────────────────────────────────────────────────────────────

fn forge_err(msg: impl Into<String>) -> WrightError {
    WrightError::ForgeError(msg.into())
}

fn forge_io(e: std::io::Error) -> WrightError {
    WrightError::IoError(e)
}

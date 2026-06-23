pub mod charge;
pub mod checkpoint;
pub mod executor;
pub mod forge;
pub mod layers;
pub mod logging;
pub mod mold;
pub mod mvp;
pub mod variables;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::info;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::foundry::charge::Charge;
use crate::foundry::executor::ExecutorRegistry;
use crate::foundry::mold::Mold;
use crate::plan::manifest::PlanManifest;

pub use crate::foundry::charge::ChargeResult;
pub use crate::foundry::forge::{Forge, ForgeContext};
pub use crate::foundry::mold::MoldResult;

#[derive(Debug)]
pub struct FoundryResult {
    pub staging_dir: PathBuf,
    pub build_root: PathBuf,
    pub logs_dir: PathBuf,
    pub output_dirs: HashMap<String, PathBuf>,
}

/// Options that control a single build invocation.
#[derive(Default)]
pub struct BuildOptions {
    pub stages: Vec<String>,
    pub force_stage: Vec<String>,
    pub until_stage: Option<String>,
    pub fetch_only: bool,
    pub skip_check: bool,
    pub force: bool,
    pub clean: bool,
    pub extra_env: HashMap<String, String>,
    pub verbose: bool,
    pub nproc_per_isolation: Option<u32>,
    pub configure_lock: Option<Arc<Semaphore>>,
    pub compile_lock: Option<Arc<Semaphore>>,
}

/// The foundry — the workshop where raw materials are transformed into
/// built artifacts.
///
/// The foundry is the micro-tier inside the **build** delivery step. It
/// orchestrates three subsystems, each with its own stages:
///
/// | Subsystem | Stages |
/// |-----------|--------|
/// | **Charge** | `fetch → verify → extract` |
/// | **Forge**  | `prepare → configure → compile → check → staging` |
/// | **Mold**   | `slice` |
///
/// Charge stages are built-in (no user scripts). Forge stages are user-defined
/// in `[pipeline.<stage>]`. Mold is a single built-in operation that divides
/// `staging/` into `outputs/<name>/` per `[[output]]` rules.
pub struct Foundry {
    config: GlobalConfig,
    executors: ExecutorRegistry,
    network_pool: Arc<Semaphore>,
}

impl Foundry {
    pub fn new(config: GlobalConfig) -> Self {
        let mut executors = ExecutorRegistry::new();
        if let Err(e) = executors.load_from_dir(&config.general.executors_dir) {
            tracing::warn!(
                "Failed to load executors from {}: {}",
                config.general.executors_dir.display(),
                e
            );
        }
        let network_pool = Arc::new(Semaphore::new(
            config.network.max_concurrent_downloads.max(1),
        ));
        Self {
            config,
            executors,
            network_pool,
        }
    }

    pub fn compute_build_key(&self, manifest: &PlanManifest) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(manifest.metadata.name.as_bytes());
        hasher.update(
            manifest
                .metadata
                .version
                .as_deref()
                .unwrap_or("")
                .as_bytes(),
        );
        hasher.update(manifest.metadata.release.to_string().as_bytes());
        for source in &manifest.sources.entries {
            match source {
                crate::plan::manifest::Source::Http(http) => {
                    hasher.update(b"http");
                    hasher.update(http.url.as_bytes());
                    hasher.update(http.sha256.as_bytes());
                    if let Some(ref r#as) = http.r#as {
                        hasher.update(r#as.as_bytes());
                    }
                    if let Some(ref ext) = http.extract_to {
                        hasher.update(ext.as_bytes());
                    }
                }
                crate::plan::manifest::Source::Git(git) => {
                    hasher.update(b"git");
                    hasher.update(git.url.as_bytes());
                    if let Some(ref r#ref) = git.r#ref {
                        hasher.update(variables::process_uri(r#ref, manifest).as_bytes());
                    }
                    if let Some(depth) = git.depth {
                        hasher.update(depth.to_le_bytes());
                    }
                    if let Some(ref ext) = git.extract_to {
                        hasher.update(ext.as_bytes());
                    }
                }
                crate::plan::manifest::Source::Local(local) => {
                    hasher.update(b"local");
                    hasher.update(local.path.as_bytes());
                    if let Some(ref ext) = local.extract_to {
                        hasher.update(ext.as_bytes());
                    }
                }
            }
        }
        let mut stage_names: Vec<_> = manifest.pipeline.keys().collect();
        stage_names.sort();
        for name in stage_names {
            if let Some(stage) = manifest.pipeline.get(name) {
                hasher.update(name.as_bytes());
                hasher.update(stage.script.as_bytes());
                hasher.update(stage.executor.as_bytes());
            }
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    pub fn build_root(&self, manifest: &PlanManifest) -> Result<PathBuf> {
        let forge_dir = if self.config.build.forge_dir.is_absolute() {
            self.config.build.forge_dir.clone()
        } else {
            std::env::current_dir()
                .map_err(|e| WrightError::ForgeError(format!("failed to get cwd: {e}")))?
                .join(&self.config.build.forge_dir)
        };
        let ver = manifest.metadata.version.as_deref().unwrap_or("");
        if ver.is_empty() {
            Ok(forge_dir.join(format!("{}-noversion", manifest.metadata.name)))
        } else {
            Ok(forge_dir.join(format!("{}-{}", manifest.metadata.name, ver)))
        }
    }

    pub async fn clean(&self, manifest: &PlanManifest) -> Result<()> {
        let build_root = self.build_root(manifest)?;
        if tokio::fs::metadata(&build_root).await.is_ok() {
            crate::foundry::layers::force_clean_dir(&build_root).await?;
            tracing::debug!("Removed forge directory: {}", build_root.display());
        }
        Ok(())
    }

    pub async fn update_hashes(&self, manifest: &PlanManifest, manifest_path: &Path) -> Result<()> {
        let charge = Charge::new(&self.config, self.network_pool.clone());
        charge.update_hashes(manifest, manifest_path).await
    }

    pub async fn build(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        base_root: &Path,
        opts: BuildOptions,
    ) -> Result<FoundryResult> {
        if opts.clean {
            self.clean(manifest).await?;
        }

        let build_root = self.build_root(manifest)?;

        // Reap any stale overlay mounts left behind by a prior crash or
        // forced termination.  This prevents EBUSY when the user later
        // deletes the build root manually.
        if tokio::fs::metadata(&build_root).await.is_ok()
            && let Err(e) = crate::foundry::layers::detach_stale_mounts(&build_root).await
        {
            tracing::warn!(event = "build.stale_mount_cleanup_failed", path = %build_root.display(), error = %e, "Failed to detach stale mounts; continuing");
        }
        let staging_dir = build_root.join("staging");
        let logs_dir = build_root.join("logs");
        let output_dir = staging_dir.clone();
        let partial = !opts.stages.is_empty() || opts.fetch_only;

        if !opts.stages.is_empty() && opts.until_stage.is_some() {
            return Err(WrightError::ForgeError(
                "cannot combine --stage with --until-stage".to_string(),
            ));
        }

        if let Some(ref stage_name) = opts.until_stage {
            let order = crate::foundry::forge::stage_order_for_manifest(
                manifest,
                opts.extra_env.get("WRIGHT_BUILD_PHASE").map(|s| s.as_str()),
            );
            if !order.iter().any(|stage| stage == stage_name) {
                return Err(WrightError::ForgeError(format!(
                    "stage '{stage_name}' not found in forge order"
                )));
            }
        }

        if !opts.stages.is_empty() {
            if !build_root.join("layers").exists() {
                return Err(WrightError::ForgeError(
                    "cannot use --stage: no previous forge found (layers/ does not exist). Run a full forge first.".to_string()
                ));
            }
            ensure_clean_dir(&staging_dir).await?;
            ensure_clean_dir(&logs_dir).await?;
        } else {
            ensure_clean_dir(&staging_dir).await?;
            ensure_clean_dir(&logs_dir).await?;
        }

        // ------------------------------------------------------------------
        // 1. Charge — source preparation
        // ------------------------------------------------------------------
        let charge = Charge::new(&self.config, self.network_pool.clone());
        let charge_result = charge.prepare(manifest, plan_dir, &build_root).await?;

        if opts.fetch_only {
            return Ok(FoundryResult {
                staging_dir,
                build_root,
                logs_dir,
                output_dirs: HashMap::new(),
            });
        }

        if let Some(ref until) = opts.until_stage
            && (until == "fetch" || until == "verify" || until == "extract")
        {
            return Ok(FoundryResult {
                staging_dir,
                build_root,
                logs_dir,
                output_dirs: HashMap::new(),
            });
        }

        // ------------------------------------------------------------------
        // 2. Forge — build execution
        // ------------------------------------------------------------------
        let rlimits = crate::isolation::ResourceLimits {
            memory_mb: manifest
                .options
                .memory_limit
                .or(self.config.build.memory_limit),
            cpu_time_secs: manifest
                .options
                .cpu_time_limit
                .or(self.config.build.cpu_time_limit),
            timeout_secs: manifest.options.timeout.or(self.config.build.timeout),
        };

        let available = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
        let total_cpus = if let Some(cap) = self.config.build.max_cpus {
            available.min(cap.max(1) as u32)
        } else {
            available
        };
        let cpu_count = opts
            .nproc_per_isolation
            .or(self.config.build.nproc_per_isolation)
            .unwrap_or(total_cpus);

        let target_dir = build_root.join("target");

        let mut vars = variables::standard_variables(variables::VariableContext {
            name: &manifest.metadata.name,
            version: manifest.metadata.version.as_deref().unwrap_or(""),
            release: manifest.metadata.release,
            arch: &manifest.metadata.arch,
            workdir: &target_dir.to_string_lossy(),
            part_dir: &staging_dir.to_string_lossy(),
            main_part_name: &manifest.metadata.name,
            main_part_dir: &staging_dir.to_string_lossy(),
        });

        for (k, v) in &manifest.options.env {
            vars.insert(k.clone(), v.clone());
        }
        vars.extend(opts.extra_env.iter().map(|(k, v)| (k.clone(), v.clone())));

        let build_key = self.compute_build_key(manifest)?;

        let mut forge = crate::foundry::forge::Forge::new(ForgeContext {
            manifest,
            source_dir: charge_result.dir,
            vars: vars.clone(),
            working_dir: &build_root,
            logs_dir: &logs_dir,
            base_root: base_root.to_path_buf(),
            work_dir: build_root.clone(),
            output_dir: output_dir.clone(),
            stages: opts.stages,
            force_stage: opts.force_stage,
            stop_after_stage: opts.until_stage,
            skip_check: opts.skip_check,
            force: opts.force,
            executors: &self.executors,
            rlimits: rlimits.clone(),
            verbose: opts.verbose,
            cpu_count: Some(cpu_count),
            configure_lock: opts.configure_lock,
            compile_cpu_count: Some(total_cpus),
            compile_lock: opts.compile_lock,
            build_key,
        })?;

        let plan_name = &manifest.metadata.name;
        let forge_t0 = std::time::Instant::now();
        info!(
            verb = "Building",
            event = "build.started",
            plan_name = %plan_name,
            "{}",
            plan_name,
        );
        if let Err(e) = forge.run().await {
            info!(event = "build.failed", plan_name = %plan_name, error = %e, "Forge failed");
            return Err(e);
        }
        let forge_elapsed = forge_t0.elapsed().as_secs_f64();
        info!(
            event = "build.completed",
            plan_name = %plan_name,
            elapsed_secs = forge_elapsed,
            "build completed"
        );

        // ------------------------------------------------------------------
        // 3. Mold — output slicing
        // ------------------------------------------------------------------
        let mold_result = if !partial {
            Mold::slice(manifest, &build_root).await?
        } else {
            MoldResult {
                default_dir: build_root.join("outputs").join("default"),
                split_dirs: HashMap::new(),
            }
        };

        Ok(FoundryResult {
            staging_dir,
            build_root,
            logs_dir,
            output_dirs: mold_result.split_dirs,
        })
    }
}

use std::collections::HashMap;

async fn ensure_clean_dir(dir: &Path) -> Result<()> {
    if tokio::fs::metadata(dir).await.is_ok() {
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                let _ = nix::mount::umount2(&dir.join("target"), nix::mount::MntFlags::MNT_DETACH);
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if let Err(e2) = tokio::fs::remove_dir_all(dir).await {
                    tracing::warn!(event = "cleanup.directory_failed", path = %dir.display(), error = %e2, reason = "stale_overlayfs", "could not clean {} (busy after retry)", dir.display());
                }
            }
            Err(e) => {
                tracing::warn!(event = "cleanup.directory_failed", path = %dir.display(), error = %e, "could not clean {}: {}", dir.display(), e);
            }
        }
    }
    tokio::fs::create_dir_all(dir).await.map_err(|e| {
        WrightError::ForgeError(format!(
            "failed to create forge directory {}: {e}",
            dir.display()
        ))
    })
}

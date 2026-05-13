use indicatif::ProgressBar;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};
use crate::forge::checkpoint::ForgeCheckpoint;
use crate::forge::executor::{self, ExecutorOptions, ExecutorRegistry};
use crate::forge::layers::LayerManager;
use crate::forge::logging;
use crate::isolation::IsolationLevel;
use crate::isolation::ResourceLimits;
use crate::plan::manifest::{PipelineStage, PlanManifest};

pub const DEFAULT_STAGES: &[&str] = &[
    "fetch",
    "verify",
    "extract",
    "prepare",
    "configure",
    "compile",
    "check",
    "staging",
];

const BUILTIN_STAGES: &[&str] = &["fetch", "verify", "extract"];

pub fn stage_order_for_manifest(manifest: &PlanManifest, build_phase: Option<&str>) -> Vec<String> {
    if build_phase == Some("mvp")
        && let Some(ref cfg) = manifest.mvp
            && let Some(ref order) = cfg.pipeline_order {
                return order.stages.clone();
            }
    if let Some(ref order) = manifest.pipeline_order {
        return order.stages.clone();
    }
    DEFAULT_STAGES.iter().map(|s| s.to_string()).collect()
}

/// Compute the full map of expected input hashes for every stage in the pipeline.
///
/// Each stage's `input_hash` chains to the previous stage's hash, forming a
/// blockchain-style hash chain.  A change to any upstream stage causes all
/// downstream hashes to differ, triggering automatic rewind.
pub fn compute_expected_hashes(
    manifest: &PlanManifest,
    stage_order: &[String],
    env: &HashMap<String, String>,
    build_phase: Option<&str>,
) -> HashMap<String, String> {
    let mut results = HashMap::new();
    let mut prev_hash = String::new();

    let is_mvp = build_phase == Some("mvp");

    for name in stage_order {
        let stage = if is_mvp {
            manifest.mvp.as_ref().and_then(|cfg| cfg.pipeline.get(name))
        } else {
            manifest.pipeline.get(name)
        };

        if let Some(stage) = stage {
            let h = ForgeCheckpoint::compute_input_hash(&stage.script, env, &prev_hash);
            results.insert(name.clone(), h.clone());
            prev_hash = h;
        }
    }

    results
}

/// Bundle of all information needed by the pipeline.
pub struct PipelineContext<'a> {
    pub manifest: &'a PlanManifest,
    pub vars: HashMap<String, String>,
    pub working_dir: &'a Path,
    pub logs_dir: &'a Path,
    pub base_root: PathBuf,
    pub work_dir: PathBuf,
    pub output_dir: PathBuf,
    pub stages: Vec<String>,
    pub force_stage: Vec<String>,
    pub stop_after_stage: Option<String>,
    pub skip_check: bool,
    pub force: bool,
    pub executors: &'a ExecutorRegistry,
    pub rlimits: ResourceLimits,
    pub verbose: bool,
    pub cpu_count: Option<u32>,
    pub configure_lock: Option<Arc<Mutex<()>>>,
    pub compile_cpu_count: Option<u32>,
    pub compile_lock: Option<Arc<Mutex<()>>>,
    pub progress: Option<ProgressBar>,
    pub build_key: String,
}

pub struct Pipeline<'a> {
    manifest: &'a PlanManifest,
    vars: HashMap<String, String>,
    logs_dir: &'a Path,
    base_root: PathBuf,
    output_dir: PathBuf,
    stages: Vec<String>,
    force_stage: Vec<String>,
    stop_after_stage: Option<String>,
    skip_check: bool,
    force: bool,
    executors: &'a ExecutorRegistry,
    rlimits: ResourceLimits,
    verbose: bool,
    cpu_count: u32,
    configure_lock: Option<Arc<Mutex<()>>>,
    compile_cpu_count: Option<u32>,
    compile_lock: Option<Arc<Mutex<()>>>,
    progress: Option<ProgressBar>,
    checkpoint: ForgeCheckpoint,
    layers: LayerManager,
    build_phase: Option<String>,
}

impl<'a> Pipeline<'a> {
    pub fn new(ctx: PipelineContext<'a>) -> Result<Self> {
        let build_phase = ctx.vars.get("WRIGHT_BUILD_PHASE").cloned();
        let plan_name = &ctx.manifest.metadata.name;
        let version = ctx.manifest.metadata.version.as_deref().unwrap_or("");

        let checkpoint = ForgeCheckpoint::load(ctx.work_dir.clone(), plan_name, version)?;
        let layers = LayerManager::new(&ctx.work_dir)?;

        Ok(Self {
            manifest: ctx.manifest,
            vars: ctx.vars,
            logs_dir: ctx.logs_dir,
            base_root: ctx.base_root,
            output_dir: ctx.output_dir,
            stages: ctx.stages,
            force_stage: ctx.force_stage,
            stop_after_stage: ctx.stop_after_stage,
            skip_check: ctx.skip_check,
            force: ctx.force,
            executors: ctx.executors,
            rlimits: ctx.rlimits,
            verbose: ctx.verbose,
            cpu_count: ctx.cpu_count.unwrap_or(1),
            configure_lock: ctx.configure_lock,
            compile_cpu_count: ctx.compile_cpu_count,
            compile_lock: ctx.compile_lock,
            progress: ctx.progress,
            checkpoint,
            layers,
            build_phase,
        })
    }

    /// Checkpoints are enabled when no explicit per-stage selection is active
    /// and `--force` is not set.
    fn can_checkpoint(&self) -> bool {
        self.stages.is_empty() && !self.force
    }

    /// Main entry: execute the full pipeline or a subset of stages.
    pub async fn run(&mut self) -> Result<()> {
        let pipeline = self.get_stage_order();

        // --stage mode: run only the selected stages (no checkpoint, no rewind).
        if !self.stages.is_empty() {
            for s in &self.stages {
                if BUILTIN_STAGES.contains(&s.as_str()) {
                    return Err(WrightError::ForgeError(format!(
                        "cannot use --stage with built-in stage '{}' (handled internally)",
                        s
                    )));
                }
                if !pipeline.iter().any(|p| p == s) {
                    return Err(WrightError::ForgeError(format!(
                        "stage '{}' not found in pipeline",
                        s
                    )));
                }
            }
            for stage_name in &pipeline {
                if self.stages.contains(stage_name) {
                    self.run_ordered_stage(stage_name).await?;
                }
            }
            return Ok(());
        }

        let stop_after_index = if let Some(ref stage_name) = self.stop_after_stage {
            Some(
                pipeline
                    .iter()
                    .position(|p| p == stage_name)
                    .ok_or_else(|| {
                        WrightError::ForgeError(format!(
                            "stage '{}' not found in pipeline",
                            stage_name
                        ))
                    })?,
            )
        } else {
            None
        };

        let checkpoint_enabled = self.can_checkpoint();

        // --- Smart resume: find where to start ---
        let start_index: usize = if checkpoint_enabled {
            let expected = compute_expected_hashes(
                self.manifest,
                &pipeline,
                &self.vars,
                self.build_phase.as_deref(),
            );
            let checkpoint_stages: Vec<String> = pipeline
                .iter()
                .filter(|stage| expected.contains_key(*stage))
                .cloned()
                .collect();
            if checkpoint_stages.is_empty() {
                0
            } else if let Some(rewind_idx) = self
                .checkpoint
                .find_rewind_point(&checkpoint_stages, &expected)
            {
                let rewind_stage = &checkpoint_stages[rewind_idx];
                let start_idx = pipeline
                    .iter()
                    .position(|stage| stage == rewind_stage)
                    .unwrap_or(0);
                info!(
                    "Smart resume: rewinding from stage '{}' (index {}) due to config change or prior failure",
                    rewind_stage, start_idx
                );
                self.checkpoint
                    .rewind_from(&checkpoint_stages, rewind_idx)?;
                self.layers.clear_layers_from(rewind_stage);
                start_idx
            } else {
                info!("All stages up-to-date — nothing to do");
                return Ok(());
            }
        } else {
            self.checkpoint.invalidate_all();
            let start_idx = pipeline
                .iter()
                .position(|stage| !BUILTIN_STAGES.contains(&stage.as_str()))
                .unwrap_or(0);
            if let Some(stage) = pipeline.get(start_idx) {
                self.layers.clear_layers_from(stage);
            }
            start_idx
        };

        // --- Execute stages from `start_index` forward ---
        for (idx, stage_name) in pipeline.iter().enumerate() {
            if idx < start_index {
                if checkpoint_enabled {
                    info!(
                        "{}",
                        logging::stage_skipped(&self.manifest.metadata.name, stage_name)
                    );
                } else {
                    debug!(
                        "{} {} handled externally (--force)",
                        logging::plan_scope(&self.manifest.metadata.name),
                        stage_name
                    );
                }
                if stop_after_index == Some(idx) {
                    return Ok(());
                }
                continue;
            }

            if BUILTIN_STAGES.contains(&stage_name.as_str()) {
                debug!("Built-in stage {} is handled by Builder", stage_name);
                if stop_after_index == Some(idx) {
                    return Ok(());
                }
                continue;
            }
            if self.skip_check && stage_name == "check" {
                debug!("Skipping check stage due to --skip-check");
                if stop_after_index == Some(idx) {
                    return Ok(());
                }
                continue;
            }

            let is_forced = self.force_stage.contains(stage_name);
            if checkpoint_enabled && !is_forced {
                let expected = compute_expected_hashes(
                    self.manifest,
                    &pipeline,
                    &self.vars,
                    self.build_phase.as_deref(),
                );
                if let Some(eh) = expected.get(stage_name)
                    && self.checkpoint.is_complete(stage_name, eh) {
                        info!(
                            "{}",
                            logging::stage_skipped(&self.manifest.metadata.name, stage_name)
                        );
                        if stop_after_index == Some(idx) {
                            return Ok(());
                        }
                        continue;
                    }
            }

            // --- Prepare layer and working directory for this stage ---
            let prev_stages: Vec<String> = pipeline[..idx].to_vec();

            self.layers.prepare_upper_layer(stage_name)?;

            // Try OverlayFS mount first; fall back to directory-based layering.
            let overlay_mounted = self.layers.mount_overlay(stage_name, &prev_stages)?;

            if !overlay_mounted {
                // Fallback: hard-link previous layers into target directory.
                self.layers.populate_target(&prev_stages)?;
            }

            // Execute the stage inside target.
            let result = self.run_ordered_stage_in_target(stage_name).await;

            self.layers.unmount_overlay();

            match result {
                Ok(()) => {
                    if !overlay_mounted {
                        // Capture delta from target into the stage's layer dir.
                        self.layers.commit_layer(stage_name, &prev_stages)?;
                    }
                    if checkpoint_enabled {
                        let expected = compute_expected_hashes(
                            self.manifest,
                            &pipeline,
                            &self.vars,
                            self.build_phase.as_deref(),
                        );
                        if let Some(eh) = expected.get(stage_name) {
                            self.checkpoint.mark_complete(stage_name, eh)?;
                        }
                    }

                    if stop_after_index == Some(idx) {
                        return Ok(());
                    }
                }
                Err(e) => {
                    if checkpoint_enabled {
                        let expected = compute_expected_hashes(
                            self.manifest,
                            &pipeline,
                            &self.vars,
                            self.build_phase.as_deref(),
                        );
                        if let Some(eh) = expected.get(stage_name) {
                            let _ = self.checkpoint.mark_failed(stage_name, eh, &e.to_string());
                        }
                    }
                    self.layers.clear_layer(stage_name);
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    /// Execute a single stage with the OverlayFS target directory as working
    /// directory.  This is like `run_ordered_stage` but uses the overlay
    /// mount point instead of the flat work directory.
    async fn run_ordered_stage_in_target(&self, stage_name: &str) -> Result<()> {
        if stage_name == "configure" {
            let _guard = if let Some(ref l) = self.configure_lock {
                Some(l.lock().await)
            } else {
                None
            };
            self.run_stage_with_hooks_in_target(stage_name, self.cpu_count)
                .await
        } else if stage_name == "compile" {
            let _guard = if let Some(ref l) = self.compile_lock {
                Some(l.lock().await)
            } else {
                None
            };
            let effective_cpu = self.compile_cpu_count.unwrap_or(self.cpu_count);
            self.run_stage_with_hooks_in_target(stage_name, effective_cpu)
                .await
        } else {
            self.run_stage_with_hooks_in_target(stage_name, self.cpu_count)
                .await
        }
    }

    async fn run_stage_with_hooks_in_target(&self, stage_name: &str, cpu_count: u32) -> Result<()> {
        let pre_hook = format!("pre_{}", stage_name);
        if let Some(stage) = self.get_stage(&pre_hook) {
            debug!("Running hook: {}", pre_hook);
            self.run_stage_in_target(&pre_hook, stage, cpu_count)
                .await?;
        }

        if let Some(stage) = self.get_stage(stage_name) {
            let t0 = std::time::Instant::now();
            if let Some(ref pb) = self.progress {
                pb.set_message(stage_name.to_string());
            }
            self.run_stage_in_target(stage_name, stage, cpu_count)
                .await?;
            let elapsed = t0.elapsed().as_secs_f64();
            info!(
                "{}",
                logging::stage_finished(&self.manifest.metadata.name, stage_name, elapsed)
            );
        } else {
            debug!("Skipping undefined stage: {}", stage_name);
        }

        let post_hook = format!("post_{}", stage_name);
        if let Some(stage) = self.get_stage(&post_hook) {
            debug!("Running hook: {}", post_hook);
            self.run_stage_in_target(&post_hook, stage, cpu_count)
                .await?;
        }

        Ok(())
    }

    fn get_stage_order(&self) -> Vec<String> {
        stage_order_for_manifest(self.manifest, self.build_phase.as_deref())
    }

    fn is_mvp_pass(&self) -> bool {
        self.build_phase.as_deref() == Some("mvp")
    }

    fn get_stage(&self, name: &str) -> Option<&PipelineStage> {
        if self.is_mvp_pass()
            && let Some(ref cfg) = self.manifest.mvp
                && let Some(stage) = cfg.pipeline.get(name) {
                    return Some(stage);
                }
        self.manifest.pipeline.get(name)
    }

    /// Legacy single-stage execution for --stage mode (no overlay layering).
    async fn run_ordered_stage(&self, stage_name: &str) -> Result<()> {
        if stage_name == "configure" {
            let _guard = if let Some(ref l) = self.configure_lock {
                Some(l.lock().await)
            } else {
                None
            };
            self.run_stage_with_hooks(stage_name, self.cpu_count).await
        } else if stage_name == "compile" {
            let _guard = if let Some(ref l) = self.compile_lock {
                Some(l.lock().await)
            } else {
                None
            };
            let effective_cpu = self.compile_cpu_count.unwrap_or(self.cpu_count);
            self.run_stage_with_hooks(stage_name, effective_cpu).await
        } else {
            self.run_stage_with_hooks(stage_name, self.cpu_count).await
        }
    }

    async fn run_stage_with_hooks(&self, stage_name: &str, cpu_count: u32) -> Result<()> {
        let pre_hook = format!("pre_{}", stage_name);
        if let Some(stage) = self.get_stage(&pre_hook) {
            debug!("Running hook: {}", pre_hook);
            self.run_stage_legacy(&pre_hook, stage, cpu_count).await?;
        }

        if let Some(stage) = self.get_stage(stage_name) {
            let t0 = std::time::Instant::now();
            if let Some(ref pb) = self.progress {
                pb.set_message(stage_name.to_string());
            }
            self.run_stage_legacy(stage_name, stage, cpu_count).await?;
            let elapsed = t0.elapsed().as_secs_f64();
            info!(
                "{}",
                logging::stage_finished(&self.manifest.metadata.name, stage_name, elapsed)
            );
        } else {
            debug!("Skipping undefined stage: {}", stage_name);
        }

        let post_hook = format!("post_{}", stage_name);
        if let Some(stage) = self.get_stage(&post_hook) {
            debug!("Running hook: {}", post_hook);
            self.run_stage_legacy(&post_hook, stage, cpu_count).await?;
        }

        Ok(())
    }

    /// Execute a stage with the overlay target as working directory.
    async fn run_stage_in_target(
        &self,
        stage_name: &str,
        stage: &PipelineStage,
        cpu_count: u32,
    ) -> Result<()> {
        if stage.script.is_empty() {
            debug!("Stage {} has empty script, skipping", stage_name);
            return Ok(());
        }

        let working_dir = self.layers.target_dir();
        let isolation_level: IsolationLevel = stage.isolation.parse()?;
        let executor = self.executors.get(&stage.executor).ok_or_else(|| {
            WrightError::ForgeError(format!("executor not found: {}", stage.executor))
        })?;

        let expanded_script = crate::forge::variables::substitute(&stage.script, &self.vars);
        let log_path = self.logs_dir.join(format!("{}.log", stage_name));

        let mut stdout_log_file = std::fs::File::create(&log_path).ok().and_then(|mut f| {
            use std::io::Write;
            let ok = write!(
                f,
                "=== Stage: {} ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n\n--- stdout ---\n",
                stage_name,
                working_dir.display(),
                expanded_script.trim()
            ).is_ok();
            if ok { Some(f) } else { None }
        });

        let stdout_log_path_owned = PathBuf::from(&log_path);

        let t0 = std::time::Instant::now();
        info!(
            "{}",
            logging::stage_started(&self.manifest.metadata.name, stage_name, isolation_level)
        );

        let max_etxtbsy_retries: u32 = 10;
        let mut attempt: u32 = 0;
        let (result, final_attempt) = loop {
            let log_stdout = if attempt > 0 {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&stdout_log_path_owned)
                    .ok()
            } else {
                stdout_log_file.take()
            };

            let mut options = ExecutorOptions {
                level: isolation_level,
                base_root: self.base_root.clone(),
                work_dir: working_dir.to_path_buf(),
                output_dir: self.output_dir.clone(),
                rlimits: self.rlimits.clone(),
                main_part_dir: None,
                verbose: self.verbose,
                cpu_count: Some(cpu_count),
                log_stdout,
                dep_mounts: Vec::new(),
            };

            let res = executor::execute_script(
                executor,
                &stage.script,
                working_dir,
                &stage.env,
                &self.vars,
                &mut options,
            )
            .await?;

            let code = res.status.code().unwrap_or(-1);
            let is_etxtbsy = code == 126
                && (res.stderr.tail.contains("Text file busy")
                    || res.stdout.tail.contains("Text file busy"));

            if is_etxtbsy && attempt < max_etxtbsy_retries {
                attempt += 1;
                let exp_base = 200_u64.saturating_mul(1_u64 << attempt.min(2)).min(1000);
                let delay_ms = exp_base + jitter_ms(exp_base);
                warn!(
                    "[{}] ETXTBUSY in stage '{}', retrying in {}ms (attempt {}/{})",
                    self.manifest.metadata.name, stage_name, delay_ms, attempt, max_etxtbsy_retries,
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                continue;
            }
            break (res, attempt);
        };

        let mut result = result;
        if final_attempt > 0 {
            info!(
                "[{}] Stage '{}' succeeded after {} ETXTBUSY retries",
                self.manifest.metadata.name, stage_name, final_attempt,
            );
        }

        let elapsed = t0.elapsed().as_secs_f64();
        let exit_code = result.status.code().unwrap_or(-1);

        if let Ok(mut log_file) = std::fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = log_file.write_all(b"\n--- stderr ---\n");
            let _ = std::io::copy(&mut result.stderr.file, &mut log_file);
            let _ = write!(
                log_file,
                "\n=== Exit code: {} ===\n=== Duration: {:.1}s ===\n",
                exit_code, elapsed
            );
        }

        if exit_code != 0 {
            return Err(WrightError::ForgeError(format!(
                "stage '{}' failed with exit code {} (see log: {})",
                stage_name,
                exit_code,
                log_path.display(),
            )));
        }

        Ok(())
    }

    /// Legacy stage execution for --stage mode (uses flat work_dir, no overlay).
    async fn run_stage_legacy(
        &self,
        stage_name: &str,
        stage: &PipelineStage,
        cpu_count: u32,
    ) -> Result<()> {
        if stage.script.is_empty() {
            debug!("Stage {} has empty script, skipping", stage_name);
            return Ok(());
        }

        let isolation_level: IsolationLevel = stage.isolation.parse()?;
        let executor = self.executors.get(&stage.executor).ok_or_else(|| {
            WrightError::ForgeError(format!("executor not found: {}", stage.executor))
        })?;

        let expanded_script = crate::forge::variables::substitute(&stage.script, &self.vars);
        let log_path = self.logs_dir.join(format!("{}.log", stage_name));

        let mut stdout_log_file = std::fs::File::create(&log_path).ok().and_then(|mut f| {
            use std::io::Write;
            let ok = write!(
                f,
                "=== Stage: {} ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n\n--- stdout ---\n",
                stage_name,
                self.layers.target_dir().display(),
                expanded_script.trim()
            ).is_ok();
            if ok { Some(f) } else { None }
        });

        let stdout_log_path_owned = PathBuf::from(&log_path);

        let t0 = std::time::Instant::now();
        info!(
            "{}",
            logging::stage_started(&self.manifest.metadata.name, stage_name, isolation_level)
        );

        let max_etxtbsy_retries: u32 = 10;
        let mut attempt: u32 = 0;
        let (result, final_attempt) = loop {
            let log_stdout = if attempt > 0 {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&stdout_log_path_owned)
                    .ok()
            } else {
                stdout_log_file.take()
            };

            let mut options = ExecutorOptions {
                level: isolation_level,
                base_root: self.base_root.clone(),
                work_dir: self.layers.target_dir().to_path_buf(),
                output_dir: self.output_dir.clone(),
                rlimits: self.rlimits.clone(),
                main_part_dir: None,
                verbose: self.verbose,
                cpu_count: Some(cpu_count),
                log_stdout,
                dep_mounts: Vec::new(),
            };

            let res = executor::execute_script(
                executor,
                &stage.script,
                self.layers.target_dir(),
                &stage.env,
                &self.vars,
                &mut options,
            )
            .await?;

            let code = res.status.code().unwrap_or(-1);
            let is_etxtbsy = code == 126
                && (res.stderr.tail.contains("Text file busy")
                    || res.stdout.tail.contains("Text file busy"));

            if is_etxtbsy && attempt < max_etxtbsy_retries {
                attempt += 1;
                let exp_base = 200_u64.saturating_mul(1_u64 << attempt.min(2)).min(1000);
                let delay_ms = exp_base + jitter_ms(exp_base);
                warn!(
                    "[{}] ETXTBUSY in stage '{}', retrying in {}ms (attempt {}/{})",
                    self.manifest.metadata.name, stage_name, delay_ms, attempt, max_etxtbsy_retries,
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                continue;
            }
            break (res, attempt);
        };

        let mut result = result;
        if final_attempt > 0 {
            info!(
                "[{}] Stage '{}' succeeded after {} ETXTBUSY retries",
                self.manifest.metadata.name, stage_name, final_attempt,
            );
        }

        let elapsed = t0.elapsed().as_secs_f64();
        let exit_code = result.status.code().unwrap_or(-1);

        if let Ok(mut log_file) = std::fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = log_file.write_all(b"\n--- stderr ---\n");
            let _ = std::io::copy(&mut result.stderr.file, &mut log_file);
            let _ = write!(
                log_file,
                "\n=== Exit code: {} ===\n=== Duration: {:.1}s ===\n",
                exit_code, elapsed
            );
        }

        if exit_code != 0 {
            return Err(WrightError::ForgeError(format!(
                "stage '{}' failed with exit code {} (see log: {})",
                stage_name,
                exit_code,
                log_path.display(),
            )));
        }

        Ok(())
    }
}

fn jitter_ms(max_ms: u64) -> u64 {
    if max_ms == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    nanos.wrapping_mul(2654435761).wrapping_add(pid) % max_ms
}

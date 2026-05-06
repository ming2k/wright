use indicatif::ProgressBar;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::builder::executor::{self, ExecutorOptions, ExecutorRegistry};
use crate::builder::logging;
use crate::error::{Result, WrightError};
use crate::isolation::IsolationLevel;
use crate::isolation::ResourceLimits;
use crate::plan::manifest::{LifecycleStage, PlanManifest};

pub const DEFAULT_STAGES: &[&str] = &[
    "fetch",
    "verify",
    "extract",
    "prepare",
    "configure",
    "compile",
    "check",
    "staging",
    "fabricate",
];

const BUILTIN_STAGES: &[&str] = &["fetch", "verify", "extract"];

const STAGING_DEPENDENT_STAGES: &[&str] = &["staging", "fabricate"];

fn stage_sentinel_path(work_dir: &Path, stage_name: &str, build_phase: Option<&str>) -> PathBuf {
    let prefix = match build_phase {
        Some("mvp") => ".wright-stage-mvp",
        _ => ".wright-stage",
    };
    work_dir.join(format!("{prefix}-{stage_name}"))
}

fn write_stage_sentinel(work_dir: &Path, stage_name: &str, build_phase: Option<&str>) {
    let path = stage_sentinel_path(work_dir, stage_name, build_phase);
    if let Err(e) = std::fs::write(&path, "") {
        warn!("Failed to write stage sentinel {}: {}", path.display(), e);
    }
}

fn has_stage_completed(work_dir: &Path, stage_name: &str, build_phase: Option<&str>) -> bool {
    stage_sentinel_path(work_dir, stage_name, build_phase).exists()
}

pub fn clean_staging_sentinels(work_dir: &Path, build_phase: Option<&str>) {
    for stage_name in STAGING_DEPENDENT_STAGES {
        let path = stage_sentinel_path(work_dir, stage_name, build_phase);
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

pub fn stage_order_for_manifest(manifest: &PlanManifest, build_phase: Option<&str>) -> Vec<String> {
    if build_phase == Some("mvp") {
        if let Some(ref cfg) = manifest.mvp {
            if let Some(ref order) = cfg.lifecycle_order {
                return order.stages.clone();
            }
        }
    }
    if let Some(ref order) = manifest.lifecycle_order {
        return order.stages.clone();
    }
    DEFAULT_STAGES.iter().map(|s| s.to_string()).collect()
}

pub struct LifecyclePipeline<'a> {
    manifest: &'a PlanManifest,
    vars: HashMap<String, String>,
    working_dir: &'a Path,
    logs_dir: &'a Path,
    base_root: PathBuf,
    work_dir: PathBuf,
    output_dir: PathBuf,
    stages: Vec<String>,
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
}

pub struct LifecycleContext<'a> {
    pub manifest: &'a PlanManifest,
    pub vars: HashMap<String, String>,
    pub working_dir: &'a Path,
    pub logs_dir: &'a Path,
    pub base_root: PathBuf,
    pub work_dir: PathBuf,
    pub output_dir: PathBuf,
    pub stages: Vec<String>,
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
}

impl<'a> LifecyclePipeline<'a> {
    pub fn new(ctx: LifecycleContext<'a>) -> Self {
        Self {
            manifest: ctx.manifest,
            vars: ctx.vars,
            working_dir: ctx.working_dir,
            logs_dir: ctx.logs_dir,
            base_root: ctx.base_root,
            work_dir: ctx.work_dir,
            output_dir: ctx.output_dir,
            stages: ctx.stages,
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
        }
    }

    fn build_phase(&self) -> Option<&str> {
        self.vars.get("WRIGHT_BUILD_PHASE").map(|s| s.as_str())
    }

    fn can_checkpoint(&self) -> bool {
        self.stages.is_empty() && !self.force
    }

    pub async fn run(&self) -> Result<()> {
        let pipeline = self.get_stage_order();

        if !self.stages.is_empty() {
            for s in &self.stages {
                if BUILTIN_STAGES.contains(&s.as_str()) {
                    return Err(WrightError::BuildError(format!(
                        "cannot use --stage with built-in stage '{}' (handled internally)",
                        s
                    )));
                }
                if !pipeline.iter().any(|p| p == s) {
                    return Err(WrightError::BuildError(format!(
                        "stage '{}' not found in lifecycle pipeline",
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

        let stop_after_index = if let Some(stage_name) = self.stop_after_stage.as_ref() {
            Some(
                pipeline
                    .iter()
                    .position(|p| p == stage_name)
                    .ok_or_else(|| {
                        WrightError::BuildError(format!(
                            "stage '{}' not found in lifecycle pipeline",
                            stage_name
                        ))
                    })?,
            )
        } else {
            None
        };

        let checkpoint_enabled = self.can_checkpoint();

        for (idx, stage_name) in pipeline.iter().enumerate() {
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

            if checkpoint_enabled
                && has_stage_completed(&self.work_dir, stage_name, self.build_phase())
            {
                info!(
                    "{}",
                    logging::stage_skipped(&self.manifest.metadata.name, stage_name)
                );
                if stop_after_index == Some(idx) {
                    return Ok(());
                }
                continue;
            }

            self.run_ordered_stage(stage_name).await?;

            if checkpoint_enabled {
                write_stage_sentinel(&self.work_dir, stage_name, self.build_phase());
            }

            if stop_after_index == Some(idx) {
                return Ok(());
            }
        }

        Ok(())
    }

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
            self.run_stage(&pre_hook, stage, cpu_count).await?;
        }

        if let Some(stage) = self.get_stage(stage_name) {
            let t0 = std::time::Instant::now();
            if let Some(ref pb) = self.progress {
                pb.set_message(stage_name.to_string());
            }
            self.run_stage(stage_name, stage, cpu_count).await?;
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
            self.run_stage(&post_hook, stage, cpu_count).await?;
        }

        Ok(())
    }

    fn get_stage_order(&self) -> Vec<String> {
        stage_order_for_manifest(
            self.manifest,
            self.vars.get("WRIGHT_BUILD_PHASE").map(|s| s.as_str()),
        )
    }

    fn is_mvp_pass(&self) -> bool {
        self.vars.get("WRIGHT_BUILD_PHASE").map(|s| s.as_str()) == Some("mvp")
    }

    fn get_stage(&self, name: &str) -> Option<&LifecycleStage> {
        if self.is_mvp_pass() {
            if let Some(ref cfg) = self.manifest.mvp {
                if let Some(stage) = cfg.lifecycle.get(name) {
                    return Some(stage);
                }
            }
        }
        self.manifest.lifecycle.get(name)
    }

    async fn run_stage(
        &self,
        stage_name: &str,
        stage: &LifecycleStage,
        cpu_count: u32,
    ) -> Result<()> {
        if stage.script.is_empty() {
            debug!("Stage {} has empty script, skipping", stage_name);
            return Ok(());
        }

        let isolation_level: IsolationLevel = stage.isolation.parse()?;
        let executor = self.executors.get(&stage.executor).ok_or_else(|| {
            WrightError::BuildError(format!("executor not found: {}", stage.executor))
        })?;

        let expanded_script = crate::builder::variables::substitute(&stage.script, &self.vars);
        let log_path = self.logs_dir.join(format!("{}.log", stage_name));

        let mut stdout_log_file = std::fs::File::create(&log_path).ok().and_then(|mut f| {
            use std::io::Write;
            let ok = write!(
                f,
                "=== Stage: {} ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n\n--- stdout ---\n",
                stage_name,
                self.working_dir.display(),
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

        // Stage-level ETXTBUSY retry.  This is the only defense that catches
        // ETXTBSY raised by *bash's* exec of a shebang interpreter (e.g. when
        // `./configure` resolves `#!/bin/sh` to the lowerdir's shared `/bin/sh`
        // inode while N parallel tasks are racing on the same overlay
        // lower-layer dentry).  The in-namespace `execvp` retry only catches
        // the top-level command and never sees this case.
        //
        // With 14 parallel tasks colliding on the shared inode, a
        // deterministic exponential backoff causes all retriers to wake
        // simultaneously and re-collide.  Capped backoff with randomized
        // jitter de-synchronizes them so they spread across the recovery
        // window instead of stacking on the same instant.
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
                work_dir: self.work_dir.clone(),
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
                self.working_dir,
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
            let output_snippet = {
                let relevant = if !result.stderr.tail.trim().is_empty() {
                    result.stderr.tail.trim()
                } else {
                    result.stdout.tail.trim()
                };
                let lines: Vec<&str> = relevant.lines().collect();
                if lines.len() > 40 {
                    format!(
                        "... ({} lines omitted) ...\n{}",
                        lines.len() - 40,
                        lines[lines.len() - 40..].join("\n")
                    )
                } else {
                    relevant.to_string()
                }
            };
            return Err(WrightError::BuildError(format!(
                "stage '{}' failed with exit code {}\nLog: {}\n\n{}",
                stage_name,
                exit_code,
                log_path.display(),
                output_snippet
            )));
        }

        Ok(())
    }
}

/// Return a pseudo-random value in `0..max_ms`, seeded from the current time.
///
/// Used to add jitter to ETXTBUSY retry backoffs so that parallel tasks
/// hitting the same shared-inode race wake up at different moments instead of
/// re-colliding at the next deterministic checkpoint.  Quality is irrelevant —
/// only de-synchronization matters — so we deliberately avoid pulling in a
/// PRNG dependency.
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

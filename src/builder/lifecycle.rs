use indicatif::ProgressBar;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

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
];

const BUILTIN_STAGES: &[&str] = &["fetch", "verify", "extract"];

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
    executors: &'a ExecutorRegistry,
    rlimits: ResourceLimits,
    verbose: bool,
    cpu_count: u32,
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
    pub executors: &'a ExecutorRegistry,
    pub rlimits: ResourceLimits,
    pub verbose: bool,
    pub cpu_count: Option<u32>,
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
            executors: ctx.executors,
            rlimits: ctx.rlimits,
            verbose: ctx.verbose,
            cpu_count: ctx.cpu_count.unwrap_or(1),
            compile_cpu_count: ctx.compile_cpu_count,
            compile_lock: ctx.compile_lock,
            progress: ctx.progress,
        }
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
                    self.run_stage_with_hooks(stage_name, self.cpu_count)
                        .await?;
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

            if stage_name == "compile" {
                let _guard = if let Some(ref l) = self.compile_lock {
                    Some(l.lock().await)
                } else {
                    None
                };
                let effective_cpu = self.compile_cpu_count.unwrap_or(self.cpu_count);
                self.run_stage_with_hooks(stage_name, effective_cpu).await?;
            } else {
                self.run_stage_with_hooks(stage_name, self.cpu_count)
                    .await?;
            }

            if stop_after_index == Some(idx) {
                return Ok(());
            }
        }

        Ok(())
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
                logging::stage_finished(&self.manifest.plan.name, stage_name, elapsed)
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

        let stdout_log_file = std::fs::File::create(&log_path).ok().and_then(|mut f| {
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

        let mut options = ExecutorOptions {
            level: isolation_level,
            base_root: self.base_root.clone(),
            work_dir: self.work_dir.clone(),
            output_dir: self.output_dir.clone(),
            rlimits: self.rlimits.clone(),
            main_part_dir: None,
            verbose: self.verbose,
            cpu_count: Some(cpu_count),
            log_stdout: stdout_log_file,
        };

        let t0 = std::time::Instant::now();
        info!(
            "{}",
            logging::stage_started(&self.manifest.plan.name, stage_name, isolation_level)
        );
        let mut result = executor::execute_script(
            executor,
            &stage.script,
            self.working_dir,
            &stage.env,
            &self.vars,
            &mut options,
        )
        .await?;
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

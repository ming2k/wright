use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indicatif::ProgressBar;
use tracing::{debug, info, warn};

use crate::builder::executor::{self, ExecutorOptions, ExecutorRegistry};
use crate::dockyard::ResourceLimits;
use crate::error::{Result, WrightError};
use crate::plan::manifest::{LifecycleStage, PlanManifest};

/// Default lifecycle pipeline order
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

/// Built-in stages handled by the build tool itself (not user scripts)
const BUILTIN_STAGES: &[&str] = &["fetch", "verify", "extract"];

const TEXT_FILE_BUSY_RETRIES: usize = 10;
const TEXT_FILE_BUSY_RETRY_DELAY: Duration = Duration::from_millis(100);

pub struct LifecyclePipeline<'a> {
    manifest: &'a PlanManifest,
    vars: HashMap<String, String>,
    working_dir: &'a Path,
    log_dir: &'a Path,
    src_dir: PathBuf,
    part_dir: PathBuf,
    files_dir: Option<PathBuf>,
    /// Stages to run; empty = run all non-builtin stages in order.
    stages: Vec<String>,
    /// Skip the `check` stage when running the default full pipeline.
    skip_check: bool,
    executors: &'a ExecutorRegistry,
    rlimits: ResourceLimits,
    verbose: bool,
    /// CPU count for non-compile stages (partitioned across active dockyards).
    /// Uses `Cell` so the compile stage can temporarily override it while
    /// holding the compile lock.
    cpu_count: Cell<Option<u32>>,
    /// CPU count used during the compile stage (= total_cpus, respecting
    /// max_cpus). `None` means inherit the partitioned cpu_count as-is.
    compile_cpu_count: Option<u32>,
    /// When set, the compile stage acquires this lock so only one dockyard
    /// compiles at a time, giving the active compile access to all capped
    /// CPU cores.
    compile_lock: Option<Arc<Mutex<()>>>,
    /// Optional spinner for live stage progress (used in multi-dockyard builds).
    progress: Option<ProgressBar>,
}

pub struct LifecycleContext<'a> {
    pub manifest: &'a PlanManifest,
    pub vars: HashMap<String, String>,
    pub working_dir: &'a Path,
    pub log_dir: &'a Path,
    pub src_dir: PathBuf,
    pub part_dir: PathBuf,
    pub files_dir: Option<PathBuf>,
    /// Stages to run; empty = run all non-builtin stages in order.
    pub stages: Vec<String>,
    /// Skip the `check` stage when running the default full pipeline.
    pub skip_check: bool,
    pub executors: &'a ExecutorRegistry,
    pub rlimits: ResourceLimits,
    pub verbose: bool,
    pub cpu_count: Option<u32>,
    /// CPU count for the compile stage (= total_cpus, respecting max_cpus).
    /// When `None`, the compile stage inherits the partitioned `cpu_count`.
    pub compile_cpu_count: Option<u32>,
    /// Compile-stage semaphore: serializes compile stages across dockyards
    /// so the active compile gets exclusive access to all capped CPU cores.
    pub compile_lock: Option<Arc<Mutex<()>>>,
    /// Optional spinner for live stage progress (used in multi-dockyard builds).
    pub progress: Option<ProgressBar>,
}

impl<'a> LifecyclePipeline<'a> {
    pub fn new(ctx: LifecycleContext<'a>) -> Self {
        Self {
            manifest: ctx.manifest,
            vars: ctx.vars,
            working_dir: ctx.working_dir,
            log_dir: ctx.log_dir,
            src_dir: ctx.src_dir,
            part_dir: ctx.part_dir,
            files_dir: ctx.files_dir,
            stages: ctx.stages,
            skip_check: ctx.skip_check,
            executors: ctx.executors,
            rlimits: ctx.rlimits,
            verbose: ctx.verbose,
            cpu_count: Cell::new(ctx.cpu_count),
            compile_cpu_count: ctx.compile_cpu_count,
            compile_lock: ctx.compile_lock,
            progress: ctx.progress,
        }
    }

    pub fn run(&self) -> Result<()> {
        let pipeline = self.get_stage_order();

        if !self.stages.is_empty() {
            // Validate requested stages
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
            // Run only the requested stages, in pipeline order
            for stage_name in &pipeline {
                if self.stages.contains(stage_name) {
                    self.run_stage_with_hooks(stage_name)?;
                }
            }
            return Ok(());
        }

        for stage_name in &pipeline {
            // Skip built-in stages (handled by Builder)
            if BUILTIN_STAGES.contains(&stage_name.as_str()) {
                debug!("Built-in stage {} is handled by Builder", stage_name);
                continue;
            }
            if self.skip_check && stage_name == "check" {
                debug!("Skipping check stage due to --skip-check");
                continue;
            }

            // Compile stages are serialized behind a semaphore so only one
            // dockyard compiles at a time, getting access to all capped CPU
            // cores (total_cpus, respecting max_cpus).
            if stage_name == "compile" {
                let _guard = self.compile_lock.as_ref().map(|l| l.lock().unwrap());
                let saved_cpu = self.cpu_count.get();
                self.cpu_count.set(self.compile_cpu_count);
                let result = self.run_stage_with_hooks(stage_name);
                self.cpu_count.set(saved_cpu);
                result?;
            } else {
                self.run_stage_with_hooks(stage_name)?;
            }
        }

        Ok(())
    }

    fn run_stage_with_hooks(&self, stage_name: &str) -> Result<()> {
        // Run pre-hook if exists
        let pre_hook = format!("pre_{}", stage_name);
        if let Some(stage) = self.get_stage(&pre_hook) {
            debug!("Running hook: {}", pre_hook);
            self.run_stage(&pre_hook, stage)?;
        }

        // Run the actual stage
        if let Some(stage) = self.get_stage(stage_name) {
            let t0 = std::time::Instant::now();
            let pkg = &self.manifest.plan.name;

            if let Some(ref pb) = self.progress {
                pb.set_message(stage_name.to_string());
            } else {
                info!("{}: running stage: {}", pkg, stage_name);
            }

            self.run_stage(stage_name, stage)?;

            let elapsed = t0.elapsed().as_secs_f64();
            if self.progress.is_some() {
                debug!("{}: stage {} finished in {:.1}s", pkg, stage_name, elapsed);
            } else {
                info!("{}: stage {} finished in {:.1}s", pkg, stage_name, elapsed);
            }
        } else {
            debug!("Skipping undefined stage: {}", stage_name);
        }

        // Run post-hook if exists
        let post_hook = format!("post_{}", stage_name);
        if let Some(stage) = self.get_stage(&post_hook) {
            debug!("Running hook: {}", post_hook);
            self.run_stage(&post_hook, stage)?;
        }

        Ok(())
    }

    fn get_stage_order(&self) -> Vec<String> {
        if self.is_mvp_pass() {
            if let Some(ref cfg) = self.manifest.mvp {
                if let Some(ref order) = cfg.lifecycle_order {
                    return order.stages.clone();
                }
            }
        }
        if let Some(ref order) = self.manifest.lifecycle_order {
            return order.stages.clone();
        }
        DEFAULT_STAGES.iter().map(|s| s.to_string()).collect()
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

    fn run_stage(&self, stage_name: &str, stage: &LifecycleStage) -> Result<()> {
        if stage.script.is_empty() {
            debug!("Stage {} has empty script, skipping", stage_name);
            return Ok(());
        }

        let executor = self.executors.get(&stage.executor).ok_or_else(|| {
            WrightError::BuildError(format!("executor not found: {}", stage.executor))
        })?;

        let options = ExecutorOptions {
            level: stage.dockyard.parse().unwrap(),
            src_dir: self.src_dir.clone(),
            part_dir: self.part_dir.clone(),
            files_dir: self.files_dir.clone(),
            rlimits: self.rlimits.clone(),
            main_part_dir: None,
            verbose: self.verbose,
            cpu_count: self.cpu_count.get(),
        };

        let t0 = std::time::Instant::now();
        let mut retries = 0;
        let mut result = loop {
            let result = executor::execute_script(
                executor,
                &stage.script,
                self.working_dir,
                &stage.env,
                &self.vars,
                &options,
            )?;

            if !should_retry_text_file_busy(&result) {
                break result;
            }

            if retries >= TEXT_FILE_BUSY_RETRIES {
                break result;
            }

            retries += 1;
            warn!(
                "{}: stage {} hit Text file busy, retrying ({}/{})",
                self.manifest.plan.name,
                stage_name,
                retries,
                TEXT_FILE_BUSY_RETRIES
            );
            std::thread::sleep(TEXT_FILE_BUSY_RETRY_DELAY);
        };
        let elapsed = t0.elapsed().as_secs_f64();

        // Write logs — stream from captured temp files to avoid holding
        // full output in memory.
        let expanded_script = crate::builder::variables::substitute(&stage.script, &self.vars);
        let log_path = self.log_dir.join(format!("{}.log", stage_name));
        if let Ok(mut log_file) = std::fs::File::create(&log_path) {
            use std::io::Write;
            let _ = write!(
                log_file,
                "=== Stage: {} ===\n=== Exit code: {} ===\n=== Duration: {:.1}s ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n",
                stage_name, result.exit_code, elapsed, self.working_dir.display(),
                expanded_script.trim()
            );
            let _ = log_file.write_all(b"--- stdout ---\n");
            let _ = std::io::copy(&mut result.stdout.file, &mut log_file);
            let _ = log_file.write_all(b"\n--- stderr ---\n");
            let _ = std::io::copy(&mut result.stderr.file, &mut log_file);
            let _ = log_file.write_all(b"\n");
        }

        if result.exit_code != 0 {
            // Many build tools (meson, cmake, autoconf) write errors to stdout.
            // Show stderr if non-empty, otherwise fall back to the tail of stdout.
            let output_snippet = {
                let relevant = if !result.stderr.tail.trim().is_empty() {
                    result.stderr.tail.trim()
                } else {
                    result.stdout.tail.trim()
                };
                // Limit to last 40 lines to keep the message readable.
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
                result.exit_code,
                log_path.display(),
                output_snippet
            )));
        }

        Ok(())
    }
}

fn should_retry_text_file_busy(result: &executor::ExecutionResult) -> bool {
    result.exit_code == 126
        && (result.stderr.tail.contains("Text file busy")
            || result.stdout.tail.contains("Text file busy"))
}

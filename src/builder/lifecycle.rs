use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use indicatif::ProgressBar;
use tracing::{debug, info};

use crate::builder::executor::{self, ExecutorOptions, ExecutorRegistry};
use crate::builder::logging;
use crate::isolation::IsolationLevel;
use crate::isolation::ResourceLimits;
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

pub struct LifecyclePipeline<'a> {
    manifest: &'a PlanManifest,
    vars: HashMap<String, String>,
    working_dir: &'a Path,
    logs_dir: &'a Path,
    base_root: PathBuf,
    work_dir: PathBuf,
    output_dir: PathBuf,
    /// Stages to run; empty = run all non-builtin stages in order.
    stages: Vec<String>,
    /// Skip the `check` stage when running the default full pipeline.
    skip_check: bool,
    executors: &'a ExecutorRegistry,
    rlimits: ResourceLimits,
    verbose: bool,
    /// CPU count for non-compile stages (partitioned across active isolations).
    /// Uses `Cell` so the compile stage can temporarily override it while
    /// holding the compile lock.
    cpu_count: Cell<Option<u32>>,
    /// CPU count used during the compile stage (= total_cpus, respecting
    /// max_cpus). `None` means inherit the partitioned cpu_count as-is.
    compile_cpu_count: Option<u32>,
    /// When set, the compile stage acquires this lock so only one isolation
    /// compiles at a time, giving the active compile access to all capped
    /// CPU cores.
    compile_lock: Option<Arc<Mutex<()>>>,
    /// Optional spinner for live stage progress (used in multi-isolation builds).
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
    /// Compile-stage semaphore: serializes compile stages across isolations
    /// so the active compile gets exclusive access to all capped CPU cores.
    pub compile_lock: Option<Arc<Mutex<()>>>,
    /// Optional spinner for live stage progress (used in multi-isolation builds).
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
            // isolation compiles at a time, getting access to all capped CPU
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

            if let Some(ref pb) = self.progress {
                pb.set_message(stage_name.to_string());
            }

            self.run_stage(stage_name, stage)?;

            let elapsed = t0.elapsed().as_secs_f64();
            info!(
                "{}",
                logging::stage_finished(&self.manifest.plan.name, stage_name, elapsed)
            );
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

        let isolation_level: IsolationLevel = stage.isolation.parse()?;

        let executor = self.executors.get(&stage.executor).ok_or_else(|| {
            WrightError::BuildError(format!("executor not found: {}", stage.executor))
        })?;

        let expanded_script = crate::builder::variables::substitute(&stage.script, &self.vars);
        let log_path = self.logs_dir.join(format!("{}.log", stage_name));

        // Open the log file before execution and write the header so that
        // stdout content is streamed into it in real time (tail -f ready).
        let stdout_log_file = std::fs::File::create(&log_path).ok().and_then(|mut f| {
            use std::io::Write;
            let ok = write!(
                f,
                "=== Stage: {} ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n\n--- stdout ---\n",
                stage_name,
                self.working_dir.display(),
                expanded_script.trim()
            )
            .is_ok();
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
            cpu_count: self.cpu_count.get(),
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
        )?;
        let elapsed = t0.elapsed().as_secs_f64();

        // Append stderr section and footer. Stdout was already streamed into
        // the log file in real time; we just need to add what's missing.
        if let Ok(mut log_file) = std::fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = log_file.write_all(b"\n--- stderr ---\n");
            let _ = std::io::copy(&mut result.stderr.file, &mut log_file);
            let _ = write!(
                log_file,
                "\n=== Exit code: {} ===\n=== Duration: {:.1}s ===\n",
                result.exit_code, elapsed
            );
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::error::{WrightError, Result};
use crate::package::manifest::{LifecycleStage, PackageManifest};
use crate::builder::executor::{self, ExecutorOptions, ExecutorRegistry};
use crate::dockyard::ResourceLimits;

/// Default lifecycle pipeline order
pub const DEFAULT_STAGES: &[&str] = &[
    "fetch", "verify", "extract", "prepare", "configure", "compile", "check", "package",
    "post_package",
];

/// Built-in stages handled by the build tool itself (not user scripts)
const BUILTIN_STAGES: &[&str] = &["fetch", "verify", "extract"];

pub struct LifecyclePipeline<'a> {
    manifest: &'a PackageManifest,
    vars: HashMap<String, String>,
    working_dir: &'a Path,
    log_dir: &'a Path,
    src_dir: PathBuf,
    pkg_dir: PathBuf,
    files_dir: Option<PathBuf>,
    /// Stages to run; empty = run all non-builtin stages in order.
    stages: Vec<String>,
    /// Skip the `check` stage when running the default full pipeline.
    skip_check: bool,
    executors: &'a ExecutorRegistry,
    rlimits: ResourceLimits,
    verbose: bool,
    cpu_count: Option<u32>,
}

pub struct LifecycleContext<'a> {
    pub manifest: &'a PackageManifest,
    pub vars: HashMap<String, String>,
    pub working_dir: &'a Path,
    pub log_dir: &'a Path,
    pub src_dir: PathBuf,
    pub pkg_dir: PathBuf,
    pub files_dir: Option<PathBuf>,
    /// Stages to run; empty = run all non-builtin stages in order.
    pub stages: Vec<String>,
    /// Skip the `check` stage when running the default full pipeline.
    pub skip_check: bool,
    pub executors: &'a ExecutorRegistry,
    pub rlimits: ResourceLimits,
    pub verbose: bool,
    pub cpu_count: Option<u32>,
}

impl<'a> LifecyclePipeline<'a> {
    pub fn new(ctx: LifecycleContext<'a>) -> Self {
        Self {
            manifest: ctx.manifest,
            vars: ctx.vars,
            working_dir: ctx.working_dir,
            log_dir: ctx.log_dir,
            src_dir: ctx.src_dir,
            pkg_dir: ctx.pkg_dir,
            files_dir: ctx.files_dir,
            stages: ctx.stages,
            skip_check: ctx.skip_check,
            executors: ctx.executors,
            rlimits: ctx.rlimits,
            verbose: ctx.verbose,
            cpu_count: ctx.cpu_count,
        }
    }

    pub fn run(&self) -> Result<()> {
        let pipeline = self.get_stage_order();

        if !self.stages.is_empty() {
            // Validate requested stages
            for s in &self.stages {
                if BUILTIN_STAGES.contains(&s.as_str()) {
                    return Err(WrightError::BuildError(format!(
                        "cannot use --stage with built-in stage '{}' (handled internally)", s
                    )));
                }
                if !pipeline.iter().any(|p| p == s) {
                    return Err(WrightError::BuildError(format!(
                        "stage '{}' not found in lifecycle pipeline", s
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
            self.run_stage_with_hooks(stage_name)?;
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
            info!("Running stage: {}", stage_name);
            self.run_stage(stage_name, stage)?;
            info!("Stage {} finished in {:.1}s", stage_name, t0.elapsed().as_secs_f64());
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

        let executor = self.executors.get(&stage.executor)
            .ok_or_else(|| WrightError::BuildError(format!("executor not found: {}", stage.executor)))?;

        let options = ExecutorOptions {
            level: stage.dockyard.parse().unwrap(),
            src_dir: self.src_dir.clone(),
            pkg_dir: self.pkg_dir.clone(),
            files_dir: self.files_dir.clone(),
            rlimits: self.rlimits.clone(),
            main_pkg_dir: None,
            verbose: self.verbose,
            cpu_count: self.cpu_count,
        };

        let t0 = std::time::Instant::now();
        let result = executor::execute_script(
            executor,
            &stage.script,
            self.working_dir,
            &stage.env,
            &self.vars,
            &options,
        )?;
        let elapsed = t0.elapsed().as_secs_f64();

        // Write logs — include the expanded script and working dir for easier debugging
        let expanded_script = crate::builder::variables::substitute(&stage.script, &self.vars);
        let log_path = self.log_dir.join(format!("{}.log", stage_name));
        let log_content = format!(
            "=== Stage: {} ===\n=== Exit code: {} ===\n=== Duration: {:.1}s ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            stage_name, result.exit_code, elapsed, self.working_dir.display(),
            expanded_script.trim(), result.stdout, result.stderr
        );
        let _ = std::fs::write(&log_path, &log_content);

        if result.exit_code != 0 {
            // Many build tools (meson, cmake, autoconf) write errors to stdout.
            // Show stderr if non-empty, otherwise fall back to the tail of stdout.
            let output_snippet = {
                let relevant = if !result.stderr.trim().is_empty() {
                    result.stderr.trim()
                } else {
                    result.stdout.trim()
                };
                // Limit to last 40 lines to keep the message readable.
                let lines: Vec<&str> = relevant.lines().collect();
                if lines.len() > 40 {
                    format!("... ({} lines omitted) ...\n{}", lines.len() - 40, lines[lines.len() - 40..].join("\n"))
                } else {
                    relevant.to_string()
                }
            };
            return Err(WrightError::BuildError(format!(
                "stage '{}' failed with exit code {}\nLog: {}\n\n{}",
                stage_name, result.exit_code, log_path.display(), output_snippet
            )));
        }

        Ok(())
    }
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::error::{WrightError, Result};
use crate::package::manifest::{LifecycleStage, PackageManifest};
use crate::builder::executor::{self, ExecutorOptions, ExecutorRegistry};
use crate::sandbox::SandboxLevel;

/// Default lifecycle pipeline order
pub const DEFAULT_STAGES: &[&str] = &[
    "fetch", "verify", "extract", "prepare", "configure", "build", "check", "package",
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
    stop_after: Option<String>,
    executors: &'a ExecutorRegistry,
}

impl<'a> LifecyclePipeline<'a> {
    pub fn new(
        manifest: &'a PackageManifest,
        vars: HashMap<String, String>,
        working_dir: &'a Path,
        log_dir: &'a Path,
        src_dir: PathBuf,
        pkg_dir: PathBuf,
        files_dir: Option<PathBuf>,
        stop_after: Option<String>,
        executors: &'a ExecutorRegistry,
    ) -> Self {
        Self {
            manifest,
            vars,
            working_dir,
            log_dir,
            src_dir,
            pkg_dir,
            files_dir,
            stop_after,
            executors,
        }
    }

    pub fn run(&self) -> Result<()> {
        let stages = self.get_stage_order();

        for stage_name in &stages {
            // Skip built-in stages (handled by Builder)
            if BUILTIN_STAGES.contains(&stage_name.as_str()) {
                info!("Built-in stage {} is handled by Builder", stage_name);
                continue;
            }

            self.run_stage_with_hooks(stage_name)?;

            // Stop after the requested stage
            if let Some(ref stop) = self.stop_after {
                if stage_name == stop {
                    info!("Stopping after stage: {}", stage_name);
                    break;
                }
            }
        }

        Ok(())
    }

    fn run_stage_with_hooks(&self, stage_name: &str) -> Result<()> {
        // Run pre-hook if exists
        let pre_hook = format!("pre_{}", stage_name);
        if let Some(stage) = self.manifest.lifecycle.get(&pre_hook) {
            info!("Running hook: {}", pre_hook);
            self.run_stage(&pre_hook, stage)?;
        }

        // Run the actual stage
        if let Some(stage) = self.manifest.lifecycle.get(stage_name) {
            info!("Running stage: {}", stage_name);
            self.run_stage(stage_name, stage)?;
        } else {
            info!("Skipping undefined stage: {}", stage_name);
        }

        // Run post-hook if exists
        let post_hook = format!("post_{}", stage_name);
        if let Some(stage) = self.manifest.lifecycle.get(&post_hook) {
            info!("Running hook: {}", post_hook);
            self.run_stage(&post_hook, stage)?;
        }

        Ok(())
    }

    fn get_stage_order(&self) -> Vec<String> {
        if let Some(ref order) = self.manifest.lifecycle_order {
            order.stages.clone()
        } else {
            DEFAULT_STAGES.iter().map(|s| s.to_string()).collect()
        }
    }

    fn run_stage(&self, stage_name: &str, stage: &LifecycleStage) -> Result<()> {
        if stage.script.is_empty() {
            info!("Stage {} has empty script, skipping", stage_name);
            return Ok(());
        }

        let executor = self.executors.get(&stage.executor)
            .ok_or_else(|| WrightError::BuildError(format!("executor not found: {}", stage.executor)))?;

        let options = ExecutorOptions {
            level: SandboxLevel::from_str(&stage.sandbox),
            src_dir: self.src_dir.clone(),
            pkg_dir: self.pkg_dir.clone(),
            files_dir: self.files_dir.clone(),
        };

        let result = executor::execute_script(
            executor,
            &stage.script,
            self.working_dir,
            &stage.env,
            &self.vars,
            &options,
        )?;

        // Write logs
        let log_path = self.log_dir.join(format!("{}.log", stage_name));
        let log_content = format!(
            "=== Stage: {} ===\n=== Exit code: {} ===\n\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            stage_name, result.exit_code, result.stdout, result.stderr
        );
        let _ = std::fs::write(&log_path, &log_content);

        if result.exit_code != 0 {
            if stage.optional {
                warn!(
                    "Optional stage '{}' failed (exit code {}), continuing",
                    stage_name, result.exit_code
                );
            } else {
                return Err(WrightError::BuildError(format!(
                    "stage '{}' failed with exit code {}\nstderr: {}",
                    stage_name, result.exit_code, result.stderr
                )));
            }
        }

        Ok(())
    }
}

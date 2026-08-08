use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info};

use crate::error::{Result, WrightError};
use crate::foundry::checkpoint::Checkpoint;
use crate::foundry::executor::ExecutorRegistry;
use crate::foundry::layers::LayerManager;
use crate::isolation::IsolationLevel;
use crate::isolation::ResourceLimits;
use wright_plan::manifest::{PipelineStage, PlanManifest};

mod execute;
mod pipeline;

use self::pipeline::manifest_stage;
pub use self::pipeline::stage_order_for_manifest;
pub(crate) use self::pipeline::{compute_expected_hashes, effective_manifest_isolation};

pub use wright_model::pipeline::DEFAULT_PIPELINE_STAGES as STAGES;

pub struct ForgeContext<'a> {
    pub manifest: &'a PlanManifest,
    pub source_dir: PathBuf,
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
    pub default_isolation: IsolationLevel,
    pub rlimits: ResourceLimits,
    pub verbose: bool,
    pub cpu_count: Option<u32>,
    pub configure_lock: Option<Arc<Semaphore>>,
    pub compile_cpu_count: Option<u32>,
    pub compile_lock: Option<Arc<Semaphore>>,
    pub build_key: String,
}

pub struct Forge<'a> {
    manifest: &'a PlanManifest,
    source_dir: PathBuf,
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
    default_isolation: IsolationLevel,
    rlimits: ResourceLimits,
    verbose: bool,
    cpu_count: u32,
    configure_lock: Option<Arc<Semaphore>>,
    compile_cpu_count: Option<u32>,
    compile_lock: Option<Arc<Semaphore>>,
    checkpoint: Checkpoint,
    layers: LayerManager,
    build_phase: Option<String>,
}

impl<'a> Forge<'a> {
    pub fn new(ctx: ForgeContext<'a>) -> Result<Self> {
        let build_phase = ctx.vars.get("WRIGHT_BUILD_PHASE").cloned();
        let plan_name = &ctx.manifest.metadata.name;
        let version = ctx.manifest.metadata.version.as_deref().unwrap_or("");

        let checkpoint = Checkpoint::load(ctx.work_dir.clone(), plan_name, version)?;
        let layers = LayerManager::new(&ctx.work_dir)?;

        Ok(Self {
            manifest: ctx.manifest,
            source_dir: ctx.source_dir,
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
            default_isolation: ctx.default_isolation,
            rlimits: ctx.rlimits,
            verbose: ctx.verbose,
            cpu_count: ctx.cpu_count.unwrap_or(1),
            configure_lock: ctx.configure_lock,
            compile_cpu_count: ctx.compile_cpu_count,
            compile_lock: ctx.compile_lock,
            checkpoint,
            layers,
            build_phase,
        })
    }

    fn can_checkpoint(&self) -> bool {
        self.stages.is_empty() && !self.force
    }

    pub async fn run(&mut self) -> Result<()> {
        let order = self.get_stage_order();

        // --stage mode: run only selected stages (no checkpoint, no rewind).
        if !self.stages.is_empty() {
            for s in &self.stages {
                if !order.iter().any(|p| p == s) {
                    return Err(WrightError::ForgeError(format!(
                        "stage '{s}' not found in forge order"
                    )));
                }
            }
            for stage_name in &order {
                if self.stages.contains(stage_name) {
                    self.run_ordered_stage(stage_name).await?;
                }
            }
            return Ok(());
        }

        let stop_after_index = if let Some(ref stage_name) = self.stop_after_stage {
            Some(order.iter().position(|p| p == stage_name).ok_or_else(|| {
                WrightError::ForgeError(format!("stage '{stage_name}' not found in forge order"))
            })?)
        } else {
            None
        };

        let checkpoint_enabled = self.can_checkpoint();
        let expected = if checkpoint_enabled {
            compute_expected_hashes(
                self.manifest,
                &order,
                &self.vars,
                self.build_phase.as_deref(),
                self.executors,
                self.default_isolation,
            )?
        } else {
            HashMap::new()
        };

        // --- Smart resume: find where to start ---
        let start_index: usize = if checkpoint_enabled {
            let checkpoint_stages: Vec<String> = order
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
                let start_idx = order
                    .iter()
                    .position(|stage| stage == rewind_stage)
                    .unwrap_or(0);
                let plan_name = &self.manifest.metadata.name;
                info!(event = "resume.smart", plan_name = %plan_name, rewind_stage = %rewind_stage, start_index = start_idx, reason = "config_change_or_prior_failure", "Smart resume triggered");
                self.checkpoint
                    .rewind_from(&checkpoint_stages, rewind_idx)?;
                self.layers.clear_layers_from(rewind_stage);
                start_idx
            } else {
                info!(event = "stage.all_up_to_date", plan_name = %self.manifest.metadata.name, "All stages up-to-date — nothing to do");
                return Ok(());
            }
        } else {
            self.checkpoint.invalidate_all();
            self.layers.clear_layers_from(&order[0]);
            0
        };

        // --- Emit one summary line for everything we'll skip up front ---
        if start_index > 0 {
            let plan_name = &self.manifest.metadata.name;
            let skipped: Vec<&str> = order[..start_index]
                .iter()
                .map(|s| s.as_str())
                .filter(|s| self.get_stage(s).is_some())
                .collect();
            if !skipped.is_empty() {
                info!(
                    verb = "Skipping",
                    event = "stage.skipped_batch",
                    plan_name = %plan_name,
                    stages = %skipped.join(", "),
                    "{} ({})",
                    skipped.join(", "),
                    plan_name,
                );
            }
        }

        // --- Execute stages from `start_index` forward ---
        for (idx, stage_name) in order.iter().enumerate() {
            if idx < start_index {
                let plan_name = &self.manifest.metadata.name;
                debug!(event = "stage.skipped", plan_name = %plan_name, stage_name = %stage_name, reason = "before_start_index", "Stage skipped");
                if stop_after_index == Some(idx) {
                    return Ok(());
                }
                continue;
            }

            if self.skip_check && stage_name == "check" {
                let plan_name = &self.manifest.metadata.name;
                debug!(event = "stage.skipped_by_flag", plan_name = %plan_name, stage_name = %stage_name, flag = "--skip-check", "Skipping check stage due to flag");
                if stop_after_index == Some(idx) {
                    return Ok(());
                }
                continue;
            }

            let is_forced = self.force_stage.contains(stage_name);
            if checkpoint_enabled
                && !is_forced
                && let Some(eh) = expected.get(stage_name)
                && self.checkpoint.is_complete(stage_name, eh)
            {
                let plan_name = &self.manifest.metadata.name;
                info!(event = "stage.skipped", plan_name = %plan_name, stage_name = %stage_name, reason = "checkpoint_up_to_date", "Stage skipped (up-to-date)");
                if stop_after_index == Some(idx) {
                    return Ok(());
                }
                continue;
            }

            // --- Prepare layer and working directory for this stage ---
            let prev_stages: Vec<String> = order[..idx].to_vec();

            self.layers.prepare_upper_layer(stage_name)?;

            let overlay_mounted =
                self.layers
                    .mount_overlay(stage_name, &self.source_dir, &prev_stages)?;

            if !overlay_mounted {
                self.layers
                    .populate_target(&self.source_dir, &prev_stages)?;
            }

            let result = self.run_ordered_stage_in_target(stage_name).await;

            self.layers.unmount_overlay();

            match result {
                Ok(()) => {
                    if !overlay_mounted {
                        self.layers
                            .commit_layer(stage_name, &self.source_dir, &prev_stages)?;
                    }
                    if checkpoint_enabled && let Some(eh) = expected.get(stage_name) {
                        self.checkpoint.mark_complete(stage_name, eh)?;
                    }
                    if stop_after_index == Some(idx) {
                        return Ok(());
                    }
                }
                Err(e) => {
                    if checkpoint_enabled && let Some(eh) = expected.get(stage_name) {
                        let _ = self.checkpoint.mark_failed(stage_name, eh, &e.to_string());
                    }
                    self.layers.clear_layer(stage_name);
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    fn get_stage_order(&self) -> Vec<String> {
        stage_order_for_manifest(self.manifest, self.build_phase.as_deref())
    }

    fn get_stage(&self, name: &str) -> Option<&PipelineStage> {
        manifest_stage(self.manifest, self.build_phase.as_deref(), name)
    }
}

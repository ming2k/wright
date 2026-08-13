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

use self::execute::StageLayering;
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
    work_dir: PathBuf,
    build_phase: Option<String>,
}

impl<'a> Forge<'a> {
    pub fn new(ctx: ForgeContext<'a>) -> Result<Self> {
        let build_phase = ctx.vars.get("WRIGHT_BUILD_PHASE").cloned();
        let plan_name = &ctx.manifest.metadata.name;
        let version = ctx.manifest.metadata.version.as_deref().unwrap_or("");

        let checkpoint = Checkpoint::load(ctx.work_dir.clone(), plan_name, version)?;
        let layers = LayerManager::new(&ctx.work_dir)?;
        let work_dir = ctx.work_dir.clone();

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
            work_dir,
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
            } else if self.staging_output_verified() {
                // All input hashes match AND the staging deliverable is
                // genuinely present in the snapshot cache. `Foundry::build`
                // wiped `staging/` before we started, so restore it now —
                // downstream consumers (mold, seal) depend on it.
                self.restore_staging()?;
                info!(
                    event = "stage.all_up_to_date",
                    plan_name = %self.manifest.metadata.name,
                    "All stages up-to-date — staging restored from cache"
                );
                return Ok(());
            } else {
                // Input hashes match but the staging output is missing or
                // corrupt on disk (hard crash mid-staging, partial `wright
                // clean`, root-owned leftover).  Rewind just the staging
                // stage so it re-runs and repopulates `staging/`.
                info!(
                    event = "resume.staging_output_invalid",
                    plan_name = %self.manifest.metadata.name,
                    "Staging output missing/corrupt despite completed checkpoint — rewinding staging"
                );
                match checkpoint_stages.iter().position(|s| s == "staging") {
                    Some(staging_ckpt_idx) => {
                        let staging_stage = checkpoint_stages[staging_ckpt_idx].clone();
                        let start_idx = order.iter().position(|s| s == &staging_stage).unwrap_or(0);
                        self.checkpoint
                            .rewind_from(&checkpoint_stages, staging_ckpt_idx)?;
                        self.layers.clear_layers_from(&staging_stage);
                        start_idx
                    }
                    None => {
                        // This plan has no staging stage — nothing to verify.
                        return Ok(());
                    }
                }
            }
        } else {
            self.checkpoint.invalidate_all();
            self.layers.clear_layers_from(&order[0]);
            0
        };

        // --- Reconcile the merged base with the checkpointed layers ---
        //
        // `base/` must equal source_dir + every checkpoint-completed stage
        // layer.  `reconcile_base` is a no-op when the manifest matches and
        // otherwise rebuilds via hard-links (resume, rewind, crash recovery).
        let mut completed: Vec<String> = order
            .iter()
            .filter(|stage| {
                expected
                    .get(*stage)
                    .is_some_and(|eh| self.checkpoint.is_complete(stage, eh))
            })
            .cloned()
            .collect();
        self.layers.reconcile_base(&self.source_dir, &completed)?;

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
                // The staging stage's deliverable lives outside the layer
                // stack, so its input hash alone cannot prove the output is
                // present.  Verify the manifest; if invalid, fall through and
                // re-run the stage instead of skipping.
                if stage_name == "staging" && !self.staging_output_verified() {
                    let plan_name = &self.manifest.metadata.name;
                    info!(
                        event = "resume.staging_output_invalid",
                        plan_name = %plan_name,
                        stage_name = %stage_name,
                        "Staging output invalid — re-running despite completed checkpoint"
                    );
                    // Fall through to the stage-execution body below.
                } else {
                    if stage_name == "staging" {
                        self.restore_staging()?;
                    }
                    let plan_name = &self.manifest.metadata.name;
                    info!(event = "stage.skipped", plan_name = %plan_name, stage_name = %stage_name, reason = "checkpoint_up_to_date", "Stage skipped (up-to-date)");
                    if stop_after_index == Some(idx) {
                        return Ok(());
                    }
                    continue;
                }
            }

            // --- Prepare layer and working directory for this stage ---
            self.layers.prepare_upper_layer(stage_name)?;

            // Pick the layering mode for the whole canonical stage (hooks
            // included): namespace-isolated stages run on a sandbox-mounted
            // overlay; a stage whose weakest hook is unisolated runs against
            // a real populated working tree instead.
            let layering = if pipeline::effective_stage_isolation(
                self.manifest,
                self.build_phase.as_deref(),
                stage_name,
                self.executors,
                self.default_isolation,
            )? == IsolationLevel::None
            {
                self.layers.populate_target()?;
                StageLayering::Fallback
            } else {
                StageLayering::Overlay(stage_name.to_string())
            };

            let result = self
                .run_ordered_stage_in_target(stage_name, &layering)
                .await;

            match result {
                Ok(()) => {
                    if matches!(layering, StageLayering::Fallback) {
                        self.layers.commit_layer(stage_name)?;
                    }
                    if expected.contains_key(stage_name) {
                        completed.push(stage_name.clone());
                    }
                    self.layers
                        .merge_layer_into_base(stage_name, &self.source_dir, &completed)?;
                    if checkpoint_enabled && let Some(eh) = expected.get(stage_name) {
                        if stage_name == "staging" {
                            // Snapshot the staging deliverable into the
                            // persistent cache and bind its manifest hash into
                            // the checkpoint.  This is what makes future
                            // resumes able to verify output integrity instead
                            // of trusting the input hash alone.
                            let manifest = self.snapshot_staging()?;
                            self.checkpoint
                                .mark_complete_with_output(stage_name, eh, manifest)?;
                        } else {
                            self.checkpoint.mark_complete(stage_name, eh)?;
                        }
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

    // ---------------------------------------------------------------
    // Staging-output integrity helpers.
    //
    // The staging stage's deliverable (`staging/`) is written by the user
    // install script outside the OverlayFS layer stack and is wiped at the
    // start of every build.  These helpers snapshot it into a persistent
    // `.staging_cache/` and verify a content manifest so that:
    //   * a skipped staging stage restores `staging/` from the cache, and
    //   * a hard-crash mid-staging (checkpoint still "complete" but output
    //     missing/corrupt) forces a re-run instead of a silent no-op.
    // ---------------------------------------------------------------

    fn staging_cache_dir(&self) -> PathBuf {
        self.work_dir.join(".staging_cache")
    }

    /// True iff the staging snapshot cache exists and its manifest matches the
    /// hash recorded in the checkpoint.  This is the single source of truth
    /// for "is the staging deliverable genuinely on disk?".
    fn staging_output_verified(&self) -> bool {
        let cache = self.staging_cache_dir();
        if !cache.exists() {
            return false;
        }
        let Some(stored) = self.checkpoint.output_manifest_hash("staging") else {
            return false;
        };
        match crate::foundry::staging_manifest::compute_dir_manifest(&cache) {
            Ok(actual) => actual == stored,
            Err(e) => {
                debug!(event = "staging.manifest_error", error = %e, "Could not compute staging manifest for verification");
                false
            }
        }
    }

    /// Snapshot `staging/` into `.staging_cache/` and return its manifest hash
    /// (or `None` when staging produced no output).
    fn snapshot_staging(&self) -> Result<Option<String>> {
        let manifest = crate::foundry::staging_manifest::compute_dir_manifest(&self.output_dir)?;
        let cache = self.staging_cache_dir();
        if manifest.is_empty() {
            if cache.exists() {
                crate::foundry::staging_manifest::remove_tree_if_exists(&cache)?;
            }
            return Ok(None);
        }
        crate::foundry::staging_manifest::snapshot_tree(&self.output_dir, &cache)?;
        Ok(Some(manifest))
    }

    /// Restore `staging/` from `.staging_cache/` (repopulating it after
    /// `Foundry::build` wiped the directory).
    fn restore_staging(&self) -> Result<()> {
        let cache = self.staging_cache_dir();
        crate::foundry::staging_manifest::restore_tree(&cache, &self.output_dir)
    }
}

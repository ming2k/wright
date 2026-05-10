use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

use crate::planning::{plan_file_fingerprint, BuildOptions};
use crate::builder::Builder;
use crate::config::GlobalConfig;
use crate::error::WrightError;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::step::{ResourceClass, Step, StepContext};

/// Stable reference to a plan file.
///
/// `content_hash` covers both `plan.toml` and any sibling `mvp.toml`, so any
/// edit that affects the build invalidates the step id and forces a rebuild.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanRef {
    pub name: String,
    pub canonical_path: PathBuf,
    pub content_hash: String,
}

impl PlanRef {
    pub fn from_path(plan_path: &std::path::Path, name: String) -> Result<Self> {
        let canonical_path = plan_path
            .canonicalize()
            .unwrap_or_else(|_| plan_path.to_path_buf());
        let content_hash = plan_file_fingerprint(plan_path)
            .map_err(|e| WorkflowError::Other(format!("plan fingerprint: {}", e)))?;
        Ok(PlanRef {
            name,
            canonical_path,
            content_hash,
        })
    }
}

/// Subset of `BuildOptions` that affects build outcomes — i.e., everything
/// except concurrency knobs. Hashed into the step id; runtime fields stay
/// out of the input record.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildOptionsCanonical {
    pub stages: Vec<String>,
    pub force_stage: Vec<String>,
    pub until_stage: Option<String>,
    pub fetch_only: bool,
    pub clean: bool,
    pub force: bool,
    pub mvp: bool,
    pub skip_check: bool,
    pub checksum: bool,
    /// When true, the step will invalidate mvp-phase checkpoints before
    /// starting the lifecycle pipeline.  Set for post-bootstrap full builds
    /// so that stages checkpointed by the preceding mvp pass are re-run.
    pub invalidate_mvp_checkpoints: bool,
}

impl BuildOptionsCanonical {
    pub fn from_options(o: &BuildOptions) -> Self {
        Self {
            stages: {
                let mut v = o.stages.clone();
                v.sort();
                v
            },
            force_stage: {
                let mut v = o.force_stage.clone();
                v.sort();
                v
            },
            until_stage: o.until_stage.clone(),
            fetch_only: o.fetch_only,
            clean: o.clean,
            force: o.force,
            mvp: o.mvp,
            skip_check: o.skip_check,
            checksum: o.checksum,
            invalidate_mvp_checkpoints: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BuildPlanInputs {
    pub plan: PlanRef,
    pub is_bootstrap: bool,
    pub bootstrap_excluded: Vec<String>,
    pub options: BuildOptionsCanonical,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BuildPlanOutputs {
    pub build_root: PathBuf,
    pub output_dirs: Vec<PathBuf>,
}

/// Build the lifecycle pipeline of a single plan.
///
/// Idempotence: the step short-circuits to success if `outputs/` is already
/// populated for every declared output and the user did not pass `--force`.
/// `LifecyclePipeline` further checkpoints individual stages via sentinel
/// files in `work_dir`.
pub struct BuildPlanStep {
    inputs: BuildPlanInputs,
    deps: Vec<StepId>,
    builder: Arc<Builder>,
    configure_lock: Arc<Mutex<()>>,
    compile_lock: Arc<Mutex<()>>,
    nproc_per_isolation: Option<u32>,
    verbose: bool,
}

impl BuildPlanStep {
    pub fn new(
        inputs: BuildPlanInputs,
        deps: Vec<StepId>,
        _config: Arc<GlobalConfig>,
        builder: Arc<Builder>,
        configure_lock: Arc<Mutex<()>>,
        compile_lock: Arc<Mutex<()>>,
        nproc_per_isolation: Option<u32>,
        verbose: bool,
    ) -> Self {
        Self {
            inputs,
            deps,
            builder,
            configure_lock,
            compile_lock,
            nproc_per_isolation,
            verbose,
        }
    }
}

impl Step for BuildPlanStep {
    type Inputs = BuildPlanInputs;
    type Outputs = BuildPlanOutputs;
    const KIND: &'static str = "build_plan";
    const RESOURCE_CLASS: ResourceClass = ResourceClass::Cpu;

    fn inputs(&self) -> &BuildPlanInputs {
        &self.inputs
    }

    fn depends_on(&self) -> &[StepId] {
        &self.deps
    }

    fn plan_name(&self) -> Option<&str> {
        Some(&self.inputs.plan.name)
    }

    fn execute(self: Arc<Self>, _ctx: StepContext) -> BoxFuture<'static, Result<BuildPlanOutputs>> {
        Box::pin(async move {
            let manifest = PlanManifest::from_file(&self.inputs.plan.canonical_path)
                .map_err(|e| WorkflowError::Other(format!("read plan: {}", e)))?;
            let build_root = self
                .builder
                .build_root(&manifest)
                .map_err(|e| WorkflowError::Other(format!("build_root: {}", e)))?;

            // Compute output dirs the workflow downstream may reference.
            let output_dirs = compute_output_dirs(&manifest, &build_root);

            if self.inputs.options.checksum {
                self.builder
                    .update_hashes(&manifest, &self.inputs.plan.canonical_path)
                    .await
                    .map_err(|e| WorkflowError::Other(format!("update_hashes: {}", e)))?;
                return Ok(BuildPlanOutputs {
                    build_root,
                    output_dirs,
                });
            }

            if self.inputs.options.clean {
                self.builder
                    .clean(&manifest)
                    .await
                    .map_err(|e| WorkflowError::Other(format!("clean: {}", e)))?;
            }

            // Intra-step idempotence: skip when staging/ is already populated.
            // (LifecyclePipeline maintains finer-grained sentinels too.)
            let opts = &self.inputs.options;
            let can_short_circuit = !opts.force
                && opts.stages.is_empty()
                && opts.until_stage.is_none()
                && !opts.fetch_only;
            if can_short_circuit && staging_is_populated(&build_root) {
                info!(
                    "{} already built; reusing populated staging/",
                    self.inputs.plan.name
                );
                return Ok(BuildPlanOutputs {
                    build_root,
                    output_dirs,
                });
            }

            // Bootstrap + MVP env shaping.
            let mut extra_env: HashMap<String, String> = HashMap::new();
            if self.inputs.is_bootstrap || opts.mvp {
                extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
                for dep in &self.inputs.bootstrap_excluded {
                    let key = format!(
                        "WRIGHT_BOOTSTRAP_WITHOUT_{}",
                        dep.to_uppercase().replace('-', "_")
                    );
                    extra_env.insert(key, "1".to_string());
                }
            } else {
                extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "full".to_string());
            }

            // A post-bootstrap full build must invalidate mvp-phase checkpoints
            // so that stages completed during the bootstrap pass are re-run
            // with the full dependency set.
            if self.inputs.options.invalidate_mvp_checkpoints {
                let work_dir = build_root.join("work");
                let ck = crate::builder::checkpoint::StageCheckpoint::new(
                    work_dir,
                    Some("mvp".to_string()),
                );
                ck.invalidate_all();
            }

            // After a bootstrap pass, clear staging checkpoints so the next
            // (full) build does not mistakenly reuse mvp staging outputs.
            if self.inputs.is_bootstrap {
                let work_dir = build_root.join("work");
                let ck = crate::builder::checkpoint::StageCheckpoint::new(
                    work_dir,
                    Some("mvp".to_string()),
                );
                ck.invalidate_from("staging");
            }

            let plan_dir = self
                .inputs
                .plan
                .canonical_path
                .parent()
                .ok_or_else(|| WorkflowError::other("plan path has no parent"))?
                .to_path_buf();

            // Effective force: a forced post-bootstrap step is conveyed by the
            // caller via inputs.options.force. We honor that directly.
            let force = opts.force;
            let nproc = self.nproc_per_isolation;
            let configure_lock = self.configure_lock.clone();
            let compile_lock = self.compile_lock.clone();
            let verbose = self.verbose;

            self.builder
                .build(
                    &manifest,
                    &plan_dir,
                    std::path::Path::new("/"),
                    &opts.stages,
                    &opts.force_stage,
                    opts.until_stage.as_deref(),
                    opts.fetch_only,
                    opts.skip_check,
                    force,
                    &extra_env,
                    verbose,
                    nproc,
                    Some(configure_lock),
                    Some(compile_lock),
                    None,
                )
                .await
                .map_err(|e: WrightError| WorkflowError::Other(format!("build: {}", e)))?;

            Ok(BuildPlanOutputs {
                build_root,
                output_dirs,
            })
        })
    }
}

fn compute_output_dirs(manifest: &PlanManifest, build_root: &std::path::Path) -> Vec<PathBuf> {
    let staging_dir = build_root.join("staging");
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => parts
            .iter()
            .map(|(sub_name, sub_part)| {
                if sub_part.include.is_none() {
                    staging_dir.clone()
                } else {
                    build_root.join("outputs").join(sub_name)
                }
            })
            .collect(),
        _ => vec![staging_dir],
    }
}

fn staging_is_populated(build_root: &std::path::Path) -> bool {
    dir_is_populated(&build_root.join("staging"))
}

fn dir_is_populated(dir: &std::path::Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return true;
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) && dir_is_populated(&p) {
                return true;
            }
        }
    }
    false
}

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::database::InstalledDb;
use crate::part::store::LocalPartStore;
use crate::transaction::{install_parts_with_explicit_targets, remove_part};
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::step::{ResourceClass, Step, StepContext};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ArchiveRef {
    pub name: String,
    pub path: PathBuf,
    pub hash: String,
}

/// Where archives come from.
///
/// Inputs are content-addressable: `Path` carries an absolute path and a
/// content hash; `FromStep` references an upstream step by id (whose own
/// id is content-addressed). This makes the install step's inputs canonical
/// in both shapes.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallSource {
    /// A locally-supplied archive path (e.g. `wright install --path …`).
    Path { path: PathBuf, hash: String },
    /// Take all archives from this upstream `PackagePlanStep`'s outputs.
    FromPackage { step: StepId },
    /// Take archives from a `ExtractPackStep` + the named part files in the
    /// pack manifest (resolved at execute time).
    FromPackExtract {
        step: StepId,
        files: Vec<String>, // part filenames inside the staging dir; sorted
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstallBatchInputs {
    pub label: String, // human-readable, e.g. "wave-3" or "install"
    pub sources: Vec<InstallSource>,
    /// Part names to mark with `Origin::Manual`. Used when the install
    /// sources are pre-resolved (`Path` / `FromPackage`).
    pub explicit_targets: Vec<String>, // sorted
    /// Pack filenames to mark with `Origin::Manual` after `FromPackExtract`
    /// sources resolve. The step reads each archive's `PARTINFO` at execute
    /// time to map filename → part name.
    pub explicit_pack_files: Vec<String>, // sorted
    pub root_dir: PathBuf,
    pub force: bool,
    pub nodeps: bool,
    /// Plans whose stale outputs (from older revisions) should be removed
    /// from the live system before installing the new batch. Sorted.
    pub plans_to_reconcile: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct InstallBatchOutputs {
    pub installed: Vec<String>,
}

/// Install a coherent batch of parts.
///
/// Idempotence: the underlying `install_parts_with_explicit_targets` skips
/// parts whose installed hash matches the incoming archive (and runs an
/// upgrade only when the archive content changed).
pub struct InstallBatchStep {
    inputs: InstallBatchInputs,
    deps: Vec<StepId>,
    part_store: Arc<LocalPartStore>,
}

impl InstallBatchStep {
    pub fn new(
        inputs: InstallBatchInputs,
        deps: Vec<StepId>,
        part_store: Arc<LocalPartStore>,
    ) -> Self {
        Self {
            inputs,
            deps,
            part_store,
        }
    }
}

impl Step for InstallBatchStep {
    type Inputs = InstallBatchInputs;
    type Outputs = InstallBatchOutputs;
    const KIND: &'static str = "install_batch";
    const RESOURCE_CLASS: ResourceClass = ResourceClass::RootMutator;

    fn inputs(&self) -> &InstallBatchInputs {
        &self.inputs
    }

    fn depends_on(&self) -> &[StepId] {
        &self.deps
    }

    fn execute(
        self: Arc<Self>,
        ctx: StepContext,
    ) -> BoxFuture<'static, Result<InstallBatchOutputs>> {
        Box::pin(async move {
            let mut paths: Vec<PathBuf> = Vec::new();
            let mut batch_part_names: Vec<String> = Vec::new();
            let mut seen: HashSet<PathBuf> = HashSet::new();

            for source in &self.inputs.sources {
                match source {
                    InstallSource::Path { path, .. } => {
                        if seen.insert(path.clone()) {
                            paths.push(path.clone());
                        }
                    }
                    InstallSource::FromPackage { step } => {
                        let v = ctx.upstream_outputs.get(step).ok_or_else(|| {
                            WorkflowError::other(format!(
                                "missing upstream package outputs for step {}",
                                step
                            ))
                        })?;
                        let outs: super::PackagePlanOutputs = serde_json::from_value(v.clone())?;
                        for a in outs.archives {
                            batch_part_names.push(a.name.clone());
                            if seen.insert(a.path.clone()) {
                                paths.push(a.path);
                            }
                        }
                    }
                    InstallSource::FromPackExtract { step, files } => {
                        let v = ctx.upstream_outputs.get(step).ok_or_else(|| {
                            WorkflowError::other(format!(
                                "missing upstream pack-extract outputs for step {}",
                                step
                            ))
                        })?;
                        let outs: super::ExtractPackOutputs = serde_json::from_value(v.clone())?;
                        for f in files {
                            let p = outs.staging_dir.join(f);
                            if seen.insert(p.clone()) {
                                paths.push(p.clone());
                            }
                            // Translate manual pack filenames -> part names by
                            // reading each archive's authoritative PARTINFO.
                            if self.inputs.explicit_pack_files.contains(f) {
                                let info = crate::part::archive::read_partinfo(&p)
                                    .map_err(|e| WorkflowError::Other(format!(
                                        "read PARTINFO from {}: {}", p.display(), e
                                    )))?;
                                batch_part_names.push(info.name);
                            }
                        }
                    }
                }
            }

            let db: &InstalledDb = &ctx.db;

            // Reconcile outdated outputs of touched plans before install.
            for plan_name in &self.inputs.plans_to_reconcile {
                let existing = db.get_parts_by_plan(plan_name).await.map_err(|e| {
                    WorkflowError::Other(format!("get_parts_by_plan: {}", e))
                })?;
                for output in existing {
                    if !batch_part_names.contains(&output.name) {
                        info!(
                            "Removing stale output {} from plan {}",
                            output.name, plan_name
                        );
                        remove_part(db, &output.name, &self.inputs.root_dir, true)
                            .await
                            .map_err(|e| {
                                WorkflowError::Other(format!(
                                    "remove stale output {}: {}",
                                    output.name, e
                                ))
                            })?;
                    }
                }
            }

            let mut explicit: HashSet<String> =
                self.inputs.explicit_targets.iter().cloned().collect();
            // For pack-extract sources, batch_part_names was populated above
            // with the names of *manual* archives; treat them as explicit too.
            for n in &batch_part_names {
                explicit.insert(n.clone());
            }
            install_parts_with_explicit_targets(
                db,
                &paths,
                &explicit,
                &self.inputs.root_dir,
                &self.part_store,
                self.inputs.force,
                self.inputs.nodeps,
            )
            .await
            .map_err(|e| WorkflowError::Other(format!("install: {}", e)))?;

            Ok(InstallBatchOutputs {
                installed: batch_part_names,
            })
        })
    }
}

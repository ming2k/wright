use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::builder::orchestrator::package_manifest;
use crate::config::GlobalConfig;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::step::{ResourceClass, Step, StepContext};

use super::install_batch::ArchiveRef;
use super::PlanRef;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PackagePlanInputs {
    pub plan: PlanRef,
    pub force: bool,
    pub out_dir: PathBuf,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PackagePlanOutputs {
    pub archives: Vec<ArchiveRef>,
}

/// Slice and pack a built plan's outputs into `.wright.tar.zst` archives.
///
/// Idempotence: `package_manifest` re-uses populated `outputs/` directories
/// and overwrites archives only when `force` is set or content has drifted.
pub struct PackagePlanStep {
    inputs: PackagePlanInputs,
    deps: Vec<StepId>,
    config: Arc<GlobalConfig>,
    print_parts: bool,
}

impl PackagePlanStep {
    pub fn new(inputs: PackagePlanInputs, deps: Vec<StepId>, config: Arc<GlobalConfig>) -> Self {
        Self {
            inputs,
            deps,
            config,
            print_parts: false,
        }
    }

    /// Print produced archive paths to stdout. Side-effect; does not affect
    /// the step's content-addressed identity.
    pub fn with_print_parts(mut self, on: bool) -> Self {
        self.print_parts = on;
        self
    }
}

impl Step for PackagePlanStep {
    type Inputs = PackagePlanInputs;
    type Outputs = PackagePlanOutputs;
    const KIND: &'static str = "package_plan";
    const RESOURCE_CLASS: ResourceClass = ResourceClass::Cpu;

    fn inputs(&self) -> &PackagePlanInputs {
        &self.inputs
    }

    fn depends_on(&self) -> &[StepId] {
        &self.deps
    }

    fn execute(
        self: Arc<Self>,
        _ctx: StepContext,
    ) -> BoxFuture<'static, Result<PackagePlanOutputs>> {
        Box::pin(async move {
            let manifest = PlanManifest::from_file(&self.inputs.plan.canonical_path)
                .map_err(|e| WorkflowError::Other(format!("read plan: {}", e)))?;
            let parts_dir = self.inputs.out_dir.clone();
            tokio::fs::create_dir_all(&parts_dir)
                .await
                .map_err(|e| WorkflowError::Other(format!("create out_dir: {}", e)))?;

            let expected = expected_archive_paths(&manifest, &parts_dir);
            let must_pack = self.inputs.force || expected.iter().any(|(_, p)| !p.exists());

            if must_pack {
                package_manifest(&manifest, &self.config, self.print_parts, self.inputs.force)
                    .await
                    .map_err(|e| WorkflowError::Other(format!("package: {}", e)))?;
            } else if self.print_parts {
                for (_, p) in &expected {
                    println!("{}", p.display());
                }
            }

            let mut archives = Vec::with_capacity(expected.len());
            for (name, path) in expected {
                if !path.exists() {
                    return Err(WorkflowError::Other(format!(
                        "expected archive not produced: {}",
                        path.display()
                    )));
                }
                let hash = crate::util::checksum::sha256_file(&path)
                    .map_err(|e| WorkflowError::Other(format!("hash archive: {}", e)))?;
                archives.push(ArchiveRef { name, path, hash });
            }
            Ok(PackagePlanOutputs { archives })
        })
    }
}

fn expected_archive_paths(
    manifest: &PlanManifest,
    parts_dir: &std::path::Path,
) -> Vec<(String, PathBuf)> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => parts
            .iter()
            .map(|(sub_name, sub_part)| {
                let sub_manifest = sub_part.to_manifest(sub_name, manifest);
                (
                    sub_name.clone(),
                    parts_dir.join(sub_manifest.part_filename()),
                )
            })
            .collect(),
        _ => vec![(
            manifest.metadata.name.clone(),
            parts_dir.join(manifest.part_filename()),
        )],
    }
}

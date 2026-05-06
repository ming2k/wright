use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::part::pack;
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::step::{ResourceClass, Step, StepContext};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExtractPackInputs {
    pub pack_sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExtractPackOutputs {
    /// Stable, content-addressed staging directory; survives process death so
    /// downstream steps can resume after a crash.
    pub staging_dir: PathBuf,
    pub manifest_json: String,
    pub overlay_present: bool,
}

/// Unpack a `.wright.pack.tar` into a stable, content-addressed staging dir.
///
/// Idempotence: the staging dir is keyed by the pack's content hash, so two
/// runs of the same pack share the same dir; the step re-extracts only if
/// the dir is missing.
pub struct ExtractPackStep {
    inputs: ExtractPackInputs,
    deps: Vec<StepId>,
    pack_path: PathBuf,
    staging_root: PathBuf,
}

impl ExtractPackStep {
    pub fn new(
        inputs: ExtractPackInputs,
        deps: Vec<StepId>,
        pack_path: PathBuf,
        staging_root: PathBuf,
    ) -> Self {
        Self {
            inputs,
            deps,
            pack_path,
            staging_root,
        }
    }
}

impl Step for ExtractPackStep {
    type Inputs = ExtractPackInputs;
    type Outputs = ExtractPackOutputs;
    const KIND: &'static str = "extract_pack";
    const RESOURCE_CLASS: ResourceClass = ResourceClass::Trivial;

    fn inputs(&self) -> &ExtractPackInputs {
        &self.inputs
    }

    fn depends_on(&self) -> &[StepId] {
        &self.deps
    }

    fn execute(
        self: Arc<Self>,
        _ctx: StepContext,
    ) -> BoxFuture<'static, Result<ExtractPackOutputs>> {
        Box::pin(async move {
            let staging_dir = self.staging_root.join(&self.inputs.pack_sha256);
            let manifest_path = staging_dir.join(pack::PACK_MANIFEST_NAME);
            let must_extract = !manifest_path.exists();

            if must_extract {
                if staging_dir.exists() {
                    let _ = std::fs::remove_dir_all(&staging_dir);
                }
                std::fs::create_dir_all(&staging_dir)
                    .map_err(|e| WorkflowError::Other(format!("mkdir staging: {}", e)))?;
                pack::extract_pack(&self.pack_path, &staging_dir)
                    .map_err(|e| WorkflowError::Other(format!("extract_pack: {}", e)))?;
            }

            let raw = std::fs::read_to_string(&manifest_path)
                .map_err(|e| WorkflowError::Other(format!("read manifest: {}", e)))?;
            let manifest = pack::parse_manifest(&raw)
                .map_err(|e| WorkflowError::Other(format!("parse manifest: {}", e)))?;

            let mismatches = pack::verify_extracted_pack(&manifest, &staging_dir)
                .map_err(|e| WorkflowError::Other(format!("verify pack: {}", e)))?;
            if !mismatches.is_empty() {
                return Err(WorkflowError::Other(format!(
                    "pack integrity check failed:\n  {}",
                    mismatches.join("\n  ")
                )));
            }

            let overlay_present = manifest.overlay_sha256.is_some();
            Ok(ExtractPackOutputs {
                staging_dir,
                manifest_json: raw,
                overlay_present,
            })
        })
    }
}

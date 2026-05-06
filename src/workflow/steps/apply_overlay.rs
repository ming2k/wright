use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::database::InstalledDb;
use crate::part::pack;
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::step::{ResourceClass, Step, StepContext};

use super::ExtractPackOutputs;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApplyOverlayInputs {
    pub root_dir: PathBuf,
    pub extract_step: StepId,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ApplyOverlayOutputs {
    pub written: usize,
}

/// Lay down a pack's `overlay.tar` over the target root, skipping any path
/// owned by an installed part (so package data is never trampled).
pub struct ApplyOverlayStep {
    inputs: ApplyOverlayInputs,
    deps: Vec<StepId>,
}

impl ApplyOverlayStep {
    pub fn new(inputs: ApplyOverlayInputs, deps: Vec<StepId>) -> Self {
        Self { inputs, deps }
    }
}

impl Step for ApplyOverlayStep {
    type Inputs = ApplyOverlayInputs;
    type Outputs = ApplyOverlayOutputs;
    const KIND: &'static str = "apply_overlay";
    const RESOURCE_CLASS: ResourceClass = ResourceClass::RootMutator;

    fn inputs(&self) -> &ApplyOverlayInputs {
        &self.inputs
    }

    fn depends_on(&self) -> &[StepId] {
        &self.deps
    }

    fn execute(
        self: Arc<Self>,
        ctx: StepContext,
    ) -> BoxFuture<'static, Result<ApplyOverlayOutputs>> {
        Box::pin(async move {
            let v = ctx
                .upstream_outputs
                .get(&self.inputs.extract_step)
                .ok_or_else(|| WorkflowError::other("missing extract step output"))?;
            let extract: ExtractPackOutputs = serde_json::from_value(v.clone())?;
            if !extract.overlay_present {
                return Ok(ApplyOverlayOutputs::default());
            }
            let overlay_tar = extract.staging_dir.join("overlay.tar");

            let db: &InstalledDb = &ctx.db;
            let owned = collect_owned_paths(db).await?;

            let written = pack::apply_overlay_tar(&overlay_tar, &self.inputs.root_dir, &owned)
                .map_err(|e| WorkflowError::Other(format!("apply_overlay_tar: {}", e)))?;
            info!("Applied {} overlay path(s)", written.len());
            Ok(ApplyOverlayOutputs {
                written: written.len(),
            })
        })
    }
}

async fn collect_owned_paths(db: &InstalledDb) -> Result<HashSet<String>> {
    let mut owned: HashSet<String> = HashSet::new();
    let parts = db
        .list_parts()
        .await
        .map_err(|e| WorkflowError::Other(format!("list parts: {}", e)))?;
    for part in parts {
        let files = db
            .get_files(part.id)
            .await
            .map_err(|e| WorkflowError::Other(format!("get_files: {}", e)))?;
        for f in files {
            owned.insert(f.path);
        }
    }
    Ok(owned)
}

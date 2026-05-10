use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::step::{ResourceClass, Step, StepContext};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ApplyConfigInputs {
    pub root_dir: PathBuf,
    pub hostname: Option<String>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub services: Vec<String>, // sorted
}

/// Apply the `[config]` block from a group manifest: hostname, timezone,
/// locale, and runit service symlinks. Pure file mutations; no DB writes.
pub struct ApplyConfigStep {
    inputs: ApplyConfigInputs,
    deps: Vec<StepId>,
}

impl ApplyConfigStep {
    pub fn new(inputs: ApplyConfigInputs, deps: Vec<StepId>) -> Self {
        Self { inputs, deps }
    }
}

impl Step for ApplyConfigStep {
    type Inputs = ApplyConfigInputs;
    type Outputs = serde_json::Value;
    const KIND: &'static str = "apply_config";
    const RESOURCE_CLASS: ResourceClass = ResourceClass::RootMutator;

    fn inputs(&self) -> &ApplyConfigInputs {
        &self.inputs
    }

    fn depends_on(&self) -> &[StepId] {
        &self.deps
    }

    fn label(&self) -> Option<&str> {
        Some("config")
    }

    fn execute(
        self: Arc<Self>,
        _ctx: StepContext,
    ) -> BoxFuture<'static, Result<serde_json::Value>> {
        Box::pin(async move {
            let root = &self.inputs.root_dir;
            if let Some(ref hostname) = self.inputs.hostname {
                let path = root.join("etc/hostname");
                std::fs::write(&path, format!("{}\n", hostname))
                    .map_err(|e| WorkflowError::Other(format!("write hostname: {}", e)))?;
            }
            if let Some(ref tz) = self.inputs.timezone {
                let target = format!("../usr/share/zoneinfo/{}", tz);
                let link = root.join("etc/localtime");
                let _ = std::fs::remove_file(&link);
                if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
                    warn!("failed to symlink {} -> {}: {}", link.display(), target, e);
                }
            }
            if let Some(ref locale) = self.inputs.locale {
                let path = root.join("etc/locale.conf");
                std::fs::write(&path, format!("LANG={}\n", locale))
                    .map_err(|e| WorkflowError::Other(format!("write locale: {}", e)))?;
            }
            if !self.inputs.services.is_empty() {
                let svc_root = root.join("var/service");
                std::fs::create_dir_all(&svc_root)
                    .map_err(|e| WorkflowError::Other(format!("mkdir var/service: {}", e)))?;
                for service in &self.inputs.services {
                    let target = format!("/etc/sv/{}", service);
                    let link = svc_root.join(service);
                    if link.exists() {
                        continue;
                    }
                    if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
                        warn!(
                            "failed to enable runit service {}: {} -> {}: {}",
                            service,
                            link.display(),
                            target,
                            e
                        );
                    }
                }
            }
            Ok(serde_json::Value::Null)
        })
    }
}

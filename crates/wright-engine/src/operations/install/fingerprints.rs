use std::collections::HashMap;

use crate::error::{Result, WrightError};
use sha2::{Digest, Sha256};
use tracing::trace;

use crate::foundry::Foundry;
use crate::resolve::BuildExecutionPlan;
use wright_plan::manifest::PlanManifest;
use wright_state::cas::CasStore;

/// Pre-computed fingerprint for each plan name in the build set.
///
/// The fingerprint captures the plan's own build key and the fingerprints of
/// its direct build dependencies, forming a content-addressed identity that
/// covers the full transitive build closure.
pub(super) struct PlanFingerprints {
    /// plan_name -> closure fingerprint
    fingerprints: HashMap<String, String>,
}

impl PlanFingerprints {
    /// Compute fingerprints for all plans in the execution plan.
    pub(super) fn compute(plan: &BuildExecutionPlan, foundry: &Foundry) -> Result<Self> {
        let mut fingerprints: HashMap<String, String> = HashMap::new();

        // Process batch by batch so that dependency fingerprints are available
        // when computing closure fingerprints for later batches.
        for batch in plan.batches() {
            for task in batch {
                let base = BuildExecutionPlan::task_base_name(task);
                let plan_path = plan
                    .plan_path_for_task(task)
                    .ok_or_else(|| WrightError::ForgeError(format!("no path for task {}", task)))?;
                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::context(format!("read plan {}", base), e))?;

                let build_key = foundry.compute_build_key(&manifest)?;

                // Collect fingerprints of build dependencies.
                let dep_names = plan.deps_for_task(task);
                let mut dep_fps: HashMap<String, String> = HashMap::new();
                for dep_name in dep_names {
                    let dep_base = BuildExecutionPlan::task_base_name(dep_name);
                    if let Some(fp) = fingerprints.get(dep_base) {
                        dep_fps.insert(dep_base.to_string(), fp.clone());
                    }
                }

                let closure_fp = CasStore::compute_closure_fingerprint(&build_key, &dep_fps);
                trace!(event = "fingerprint.closure", plan_name = %base, closure_fp = %&closure_fp[..8], "Computed closure fingerprint");

                // Insert for both the full task and its bootstrap variant.
                // Bootstrap tasks get a different fingerprint to distinguish
                // from full builds (different compilation results).
                fingerprints.insert(base.to_string(), closure_fp.clone());
                if task.ends_with(":bootstrap") {
                    fingerprints.insert(task.clone(), closure_fp);
                } else {
                    // Also insert the :bootstrap variant if it exists.
                    let bootstrap_task = format!("{}:bootstrap", base);
                    if plan.build_set().contains(&bootstrap_task) {
                        let mut bp = Sha256::new();
                        bp.update(closure_fp.as_bytes());
                        bp.update(b":bootstrap");
                        let bootstrap_fp = format!("{:x}", bp.finalize());
                        trace!(event = "fingerprint.bootstrap", plan_name = %base, bootstrap_fp = %&bootstrap_fp[..8], "Computed bootstrap fingerprint");
                        fingerprints.insert(bootstrap_task, bootstrap_fp);
                    }
                }
            }
        }

        Ok(Self { fingerprints })
    }

    pub(super) fn get(&self, name: &str) -> Option<&String> {
        self.fingerprints.get(name)
    }
}

use std::path::Path;

use crate::config::GlobalConfig;
use crate::resolve::{BuildPlanOptions, DepDomain, MatchPolicy};
use wright_part::store::LocalPartStore;

pub struct InstallRequest<'a> {
    pub targets: Vec<String>,
    /// Forward dependency domain to expand (empty = targets only).
    pub deps: DepDomain,
    /// Reverse-dependency (dependents) domain to expand for deployed parts
    /// (empty = no rdeps expansion).
    pub rdeps: DepDomain,
    pub match_policies: Vec<MatchPolicy>,
    pub depth: Option<usize>,
    pub force: bool,
    /// Clear the forge workspace, including source/work trees. Distinct from
    /// `force`: `clean` forces a from-scratch forge without redeploying parts
    /// that are already converged. `force` implies clean.
    pub clean: bool,
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub root_dir: &'a Path,
    pub verbose: u8,
    pub quiet: bool,
    pub part_store: &'a LocalPartStore,
    /// Optional forge options. When provided, the install flow uses these
    /// instead of default BuildPlanOptions (used by `wright build`).
    pub build_opts: Option<BuildPlanOptions>,
    pub run_hooks: bool,
    /// Resolve and print the execution plan, then return without forging,
    /// sealing, or deploying anything.
    pub dry_run: bool,
}

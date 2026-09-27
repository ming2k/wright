use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use crate::error::{Result, SchedulerError};

/// Unique identifier for an action in the execution graph (ADR-0048).
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct ActionId(pub String);

impl ActionId {
    pub fn new(package: &str, verb: &str) -> Self {
        Self(format!("{}:{}", package, verb))
    }

    pub fn lint(package: &str) -> Self {
        Self::new(package, "lint")
    }

    pub fn fetch(package: &str) -> Self {
        Self::new(package, "fetch")
    }

    pub fn build(package: &str) -> Self {
        Self::new(package, "build")
    }

    pub fn restore_cache(package: &str) -> Self {
        Self::new(package, "restore_cache")
    }

    pub fn seal(package: &str) -> Self {
        Self::new(package, "seal")
    }

    pub fn verify_abi(package: &str) -> Self {
        Self::new(package, "verify_abi")
    }

    pub fn deploy(package: &str) -> Self {
        Self::new(package, "deploy")
    }

    pub fn commit(package: &str) -> Self {
        Self::new(package, "commit")
    }

    pub fn rollback(package: &str) -> Self {
        Self::new(package, "rollback")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The 9 canonical action atoms defined in ADR-0048.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
    /// Source tree validation.
    Lint { plan_name: String },

    /// Source acquisition and checksum verification.
    Fetch { plan_name: String },

    /// Isolated sandbox compilation producing staging tree.
    Build {
        plan_name: String,
        clean: bool,
        force: bool,
        mvp: bool,
    },

    /// Zero-second cache bypass: restore part archives from BuildCache.
    RestoreCache {
        plan_name: String,
        fingerprint: String,
    },

    /// Slice staging tree and seal into `.wright.tar.zst` archives.
    Seal {
        plan_name: String,
        force: bool,
    },

    /// Extract and diff ELF ABI symbols to decide dynamic downstream pruning.
    VerifyAbi { plan_name: String },

    /// Transactionally merge part archives onto the live root filesystem.
    Deploy {
        plan_name: String,
        archive_paths: Vec<PathBuf>,
    },

    /// Record package registration in database registry and file ledger.
    CommitRegistry { plan_name: String },

    /// Saga compensating action to revert partially applied system changes.
    Rollback {
        plan_name: String,
        archive_paths: Vec<PathBuf>,
    },
}

impl ActionKind {
    pub fn plan_name(&self) -> &str {
        match self {
            Self::Lint { plan_name }
            | Self::Fetch { plan_name }
            | Self::Build { plan_name, .. }
            | Self::RestoreCache { plan_name, .. }
            | Self::Seal { plan_name, .. }
            | Self::VerifyAbi { plan_name }
            | Self::Deploy { plan_name, .. }
            | Self::CommitRegistry { plan_name }
            | Self::Rollback { plan_name, .. } => plan_name,
        }
    }

    pub fn verb(&self) -> &'static str {
        match self {
            Self::Lint { .. } => "Lint",
            Self::Fetch { .. } => "Fetch",
            Self::Build { .. } => "Build",
            Self::RestoreCache { .. } => "RestoreCache",
            Self::Seal { .. } => "Seal",
            Self::VerifyAbi { .. } => "VerifyAbi",
            Self::Deploy { .. } => "Deploy",
            Self::CommitRegistry { .. } => "Commit",
            Self::Rollback { .. } => "Rollback",
        }
    }

    /// Resource demands for the scheduler's concurrency control.
    pub fn resource_demand(&self, default_cpus: usize) -> ResourceDemand {
        match self {
            Self::Build { .. } => ResourceDemand {
                cpus: default_cpus,
                needs_configure_lock: true,
                needs_root_lock: false,
            },
            Self::Deploy { .. } | Self::Rollback { .. } => ResourceDemand {
                cpus: 1,
                needs_configure_lock: false,
                needs_root_lock: true,
            },
            _ => ResourceDemand {
                cpus: 1,
                needs_configure_lock: false,
                needs_root_lock: false,
            },
        }
    }
}

/// Concurrency requirements for an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceDemand {
    pub cpus: usize,
    pub needs_configure_lock: bool,
    pub needs_root_lock: bool,
}

/// Execution status of an action node in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionStatus {
    Pending,
    Ready,
    Running,
    Succeeded,
    Failed(String),
    Skipped(String),
}

impl ActionStatus {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed(_) | Self::Skipped(_)
        )
    }

    pub fn is_success_or_skipped(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Skipped(_))
    }
}

/// A node in the Action DAG.
#[derive(Debug, Clone)]
pub struct ActionNode {
    pub id: ActionId,
    pub package_name: String,
    pub kind: ActionKind,
    pub status: ActionStatus,
}

impl ActionNode {
    pub fn new(id: ActionId, package_name: impl Into<String>, kind: ActionKind) -> Self {
        Self {
            id,
            package_name: package_name.into(),
            kind,
            status: ActionStatus::Pending,
        }
    }
}

/// The physical Action DAG (ADR-0048).
///
/// Nodes represent atomic actions; directed edges represent physical causal
/// dependencies (`dependent -> prerequisite`).
#[derive(Debug, Clone, Default)]
pub struct ActionGraph {
    nodes: HashMap<ActionId, ActionNode>,
    /// Maps action -> prerequisites that must complete before it can run.
    dependencies: HashMap<ActionId, HashSet<ActionId>>,
    /// Maps action -> downstream dependents waiting for it to complete.
    dependents: HashMap<ActionId, HashSet<ActionId>>,
}

impl ActionGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn add_node(&mut self, node: ActionNode) {
        let id = node.id.clone();
        self.nodes.insert(id.clone(), node);
        self.dependencies.entry(id.clone()).or_default();
        self.dependents.entry(id).or_default();
    }

    pub fn get(&self, id: &ActionId) -> Option<&ActionNode> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: &ActionId) -> Option<&mut ActionNode> {
        self.nodes.get_mut(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &ActionNode> {
        self.nodes.values()
    }

    /// Add a dependency edge: `dependent` cannot run until `prerequisite` completes.
    pub fn add_dependency(&mut self, dependent: &ActionId, prerequisite: &ActionId) -> Result<()> {
        if !self.nodes.contains_key(dependent) {
            return Err(SchedulerError::NodeNotFound(dependent.to_string()));
        }
        if !self.nodes.contains_key(prerequisite) {
            return Err(SchedulerError::NodeNotFound(prerequisite.to_string()));
        }
        self.dependencies
            .entry(dependent.clone())
            .or_default()
            .insert(prerequisite.clone());
        self.dependents
            .entry(prerequisite.clone())
            .or_default()
            .insert(dependent.clone());
        Ok(())
    }

    /// Direct prerequisites of an action.
    pub fn dependencies_of(&self, id: &ActionId) -> Option<&HashSet<ActionId>> {
        self.dependencies.get(id)
    }

    /// Direct dependents waiting on an action.
    pub fn dependents_of(&self, id: &ActionId) -> Option<&HashSet<ActionId>> {
        self.dependents.get(id)
    }

    /// Retrieve all actions that are currently in `Pending` state and whose
    /// prerequisites have all finished successfully or been skipped.
    pub fn ready_actions(&self) -> Vec<ActionId> {
        let mut ready = Vec::new();
        for (id, node) in &self.nodes {
            if node.status != ActionStatus::Pending {
                continue;
            }
            let deps = self.dependencies.get(id);
            let all_deps_satisfied = match deps {
                None => true,
                Some(prereqs) => prereqs.iter().all(|prereq_id| {
                    self.nodes
                        .get(prereq_id)
                        .map(|n| n.status.is_success_or_skipped())
                        .unwrap_or(false)
                }),
            };
            if all_deps_satisfied {
                ready.push(id.clone());
            }
        }
        ready.sort();
        ready
    }

    pub fn mark_status(&mut self, id: &ActionId, status: ActionStatus) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = status;
        }
    }

    /// Mark an action as running.
    pub fn mark_running(&mut self, id: &ActionId) {
        self.mark_status(id, ActionStatus::Running);
    }

    /// Mark an action as succeeded.
    pub fn mark_succeeded(&mut self, id: &ActionId) {
        self.mark_status(id, ActionStatus::Succeeded);
    }

    /// Mark an action as failed.
    pub fn mark_failed(&mut self, id: &ActionId, error: impl Into<String>) {
        self.mark_status(id, ActionStatus::Failed(error.into()));
    }

    /// Mark an action as skipped (e.g. via ABI probe inhibition or cache hit).
    pub fn mark_skipped(&mut self, id: &ActionId, reason: impl Into<String>) {
        self.mark_status(id, ActionStatus::Skipped(reason.into()));
    }

    /// Dynamically prune an action and recursively skip all downstream actions
    /// that solely depended on it.
    pub fn prune_subtree(&mut self, root_id: &ActionId, reason: &str) {
        let mut queue = vec![root_id.clone()];
        while let Some(current) = queue.pop() {
            if let Some(node) = self.nodes.get_mut(&current) {
                if !node.status.is_finished() {
                    node.status = ActionStatus::Skipped(reason.to_string());
                    if let Some(children) = self.dependents.get(&current) {
                        for child in children {
                            queue.push(child.clone());
                        }
                    }
                }
            }
        }
    }

    /// Check whether all actions in the graph have completed (succeeded, failed, or skipped).
    pub fn is_finished(&self) -> bool {
        self.nodes.values().all(|n| n.status.is_finished())
    }

    /// Check whether any action in the graph has failed.
    pub fn has_failures(&self) -> bool {
        self.nodes
            .values()
            .any(|n| matches!(n.status, ActionStatus::Failed(_)))
    }

    /// List all failed actions with their error messages.
    pub fn failures(&self) -> Vec<(&ActionId, &str)> {
        self.nodes
            .iter()
            .filter_map(|(id, n)| match &n.status {
                ActionStatus::Failed(err) => Some((id, err.as_str())),
                _ => None,
            })
            .collect()
    }

    /// Check if all actions associated with a specific package have succeeded or skipped.
    pub fn is_package_complete(&self, package_name: &str) -> bool {
        let pkg_nodes: Vec<_> = self
            .nodes
            .values()
            .filter(|n| n.package_name == package_name)
            .collect();
        !pkg_nodes.is_empty() && pkg_nodes.iter().all(|n| n.status.is_success_or_skipped())
    }

    /// Topologically sorted order of actions for validation and execution inspection.
    pub fn topological_sort(&self) -> Result<Vec<ActionId>> {
        let mut in_degree: HashMap<ActionId, usize> = HashMap::new();
        for id in self.nodes.keys() {
            let count = self.dependencies.get(id).map(|s| s.len()).unwrap_or(0);
            in_degree.insert(id.clone(), count);
        }

        let mut queue: Vec<ActionId> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| id.clone())
            .collect();
        queue.sort();

        let mut order = Vec::with_capacity(self.nodes.len());

        while let Some(curr) = queue.pop() {
            order.push(curr.clone());
            if let Some(children) = self.dependents.get(&curr) {
                let mut next_batch = Vec::new();
                for child in children {
                    if let Some(deg) = in_degree.get_mut(child) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            next_batch.push(child.clone());
                        }
                    }
                }
                next_batch.sort();
                queue.extend(next_batch);
            }
        }

        if order.len() != self.nodes.len() {
            return Err(SchedulerError::CycleDetected);
        }

        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_topological_sort_and_ready_actions() {
        let mut graph = ActionGraph::new();

        let a_build = ActionId::build("pkgA");
        let a_seal = ActionId::seal("pkgA");
        let a_deploy = ActionId::deploy("pkgA");
        let b_build = ActionId::build("pkgB");

        graph.add_node(ActionNode::new(
            a_build.clone(),
            "pkgA",
            ActionKind::Build {
                plan_name: "pkgA".into(),
                clean: false,
                force: false,
                mvp: false,
            },
        ));
        graph.add_node(ActionNode::new(
            a_seal.clone(),
            "pkgA",
            ActionKind::Seal {
                plan_name: "pkgA".into(),
                force: false,
            },
        ));
        graph.add_node(ActionNode::new(
            a_deploy.clone(),
            "pkgA",
            ActionKind::Deploy {
                plan_name: "pkgA".into(),
                archive_paths: vec![],
            },
        ));
        graph.add_node(ActionNode::new(
            b_build.clone(),
            "pkgB",
            ActionKind::Build {
                plan_name: "pkgB".into(),
                clean: false,
                force: false,
                mvp: false,
            },
        ));

        // Package A internal pipeline: build -> seal -> deploy
        graph.add_dependency(&a_seal, &a_build).unwrap();
        graph.add_dependency(&a_deploy, &a_seal).unwrap();

        // Cross-package point-to-point pipeline: pkgB depends on pkgA deployed
        graph.add_dependency(&b_build, &a_deploy).unwrap();

        let order = graph.topological_sort().unwrap();
        assert_eq!(order.len(), 4);
        assert_eq!(order[0], a_build);

        // Initially only a_build is ready
        assert_eq!(graph.ready_actions(), vec![a_build.clone()]);

        // Complete a_build
        graph.mark_succeeded(&a_build);
        assert_eq!(graph.ready_actions(), vec![a_seal.clone()]);

        // Complete a_seal
        graph.mark_succeeded(&a_seal);
        assert_eq!(graph.ready_actions(), vec![a_deploy.clone()]);

        // Complete a_deploy -> triggers b_build and completes pkgA!
        graph.mark_succeeded(&a_deploy);
        assert!(graph.is_package_complete("pkgA"));
        assert_eq!(graph.ready_actions(), vec![b_build.clone()]);

        // Complete b_build
        graph.mark_succeeded(&b_build);
        assert!(graph.is_finished());
        assert!(!graph.has_failures());
    }

    #[test]
    fn graph_prune_subtree() {
        let mut graph = ActionGraph::new();
        let a = ActionId::build("pkgA");
        let b = ActionId::build("pkgB");
        let c = ActionId::build("pkgC");

        graph.add_node(ActionNode::new(
            a.clone(),
            "pkgA",
            ActionKind::Build {
                plan_name: "pkgA".into(),
                clean: false,
                force: false,
                mvp: false,
            },
        ));
        graph.add_node(ActionNode::new(
            b.clone(),
            "pkgB",
            ActionKind::Build {
                plan_name: "pkgB".into(),
                clean: false,
                force: false,
                mvp: false,
            },
        ));
        graph.add_node(ActionNode::new(
            c.clone(),
            "pkgC",
            ActionKind::Build {
                plan_name: "pkgC".into(),
                clean: false,
                force: false,
                mvp: false,
            },
        ));

        graph.add_dependency(&b, &a).unwrap();
        graph.add_dependency(&c, &b).unwrap();

        // Pruning a should skip a, b, and c
        graph.prune_subtree(&a, "ABI backward compatible");
        assert!(matches!(graph.get(&a).unwrap().status, ActionStatus::Skipped(_)));
        assert!(matches!(graph.get(&b).unwrap().status, ActionStatus::Skipped(_)));
        assert!(matches!(graph.get(&c).unwrap().status, ActionStatus::Skipped(_)));
        assert!(graph.is_finished());
    }
}

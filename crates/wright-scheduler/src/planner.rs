use std::collections::HashSet;

use crate::error::Result;
use super::graph::{ActionGraph, ActionId, ActionKind, ActionNode};

/// Minimal domain interface required to compile a package execution plan into an Action DAG.
pub trait PackageGraphPlan {
    fn build_set(&self) -> &HashSet<String>;
    fn deps_for_task(&self, task: &str) -> &[String];
    fn is_post_bootstrap_full(&self, task: &str) -> bool;
}

/// Configuration options for lowering a plan into an Action DAG.
#[derive(Debug, Clone, Default)]
pub struct PlannerOptions {
    pub clean: bool,
    pub force: bool,
    pub mvp: bool,
    pub skip_deploy: bool,
}

/// Compiles declarative Package DAGs into fine-grained Action DAGs (ADR-0048).
pub struct ActionPlanner;

impl ActionPlanner {
    /// Lower a resolved package plan into an `ActionGraph` with
    /// point-to-point cross-package pipelining edges.
    pub fn plan<P: PackageGraphPlan>(
        exec_plan: &P,
        opts: &PlannerOptions,
    ) -> Result<ActionGraph> {
        let mut graph = ActionGraph::new();

        // 1. Instantiate intra-package action atoms for each task.
        for task in exec_plan.build_set() {
            let is_bootstrap = task.ends_with(":bootstrap");
            let force = if !is_bootstrap && exec_plan.is_post_bootstrap_full(task) {
                true
            } else {
                opts.force
            };

            let build_id = ActionId::build(task);
            let seal_id = ActionId::seal(task);
            let verify_id = ActionId::verify_abi(task);

            graph.add_node(ActionNode::new(
                build_id.clone(),
                task,
                ActionKind::Build {
                    plan_name: task.clone(),
                    clean: opts.clean,
                    force,
                    mvp: opts.mvp || is_bootstrap,
                },
            ));

            graph.add_node(ActionNode::new(
                seal_id.clone(),
                task,
                ActionKind::Seal {
                    plan_name: task.clone(),
                    force,
                },
            ));
            graph.add_dependency(&seal_id, &build_id)?;

            graph.add_node(ActionNode::new(
                verify_id.clone(),
                task,
                ActionKind::VerifyAbi {
                    plan_name: task.clone(),
                },
            ));
            graph.add_dependency(&verify_id, &seal_id)?;

            if !opts.skip_deploy {
                let deploy_id = ActionId::deploy(task);
                let commit_id = ActionId::commit(task);

                graph.add_node(ActionNode::new(
                    deploy_id.clone(),
                    task,
                    ActionKind::Deploy {
                        plan_name: task.clone(),
                        archive_paths: Vec::new(),
                    },
                ));
                graph.add_dependency(&deploy_id, &verify_id)?;

                graph.add_node(ActionNode::new(
                    commit_id.clone(),
                    task,
                    ActionKind::CommitRegistry {
                        plan_name: task.clone(),
                    },
                ));
                graph.add_dependency(&commit_id, &deploy_id)?;
            }
        }

        // 2. Connect cross-package point-to-point pipelining edges.
        //    If Package B depends on Package A:
        //    - Normal install: Build(B) cannot start until Deploy(A) finishes.
        //    - Build-only: Build(B) cannot start until Seal(A) finishes.
        for task in exec_plan.build_set() {
            let build_id = ActionId::build(task);
            for dep in exec_plan.deps_for_task(task) {
                if exec_plan.build_set().contains(dep) {
                    let prereq_id = if opts.skip_deploy {
                        ActionId::seal(dep)
                    } else {
                        ActionId::deploy(dep)
                    };
                    graph.add_dependency(&build_id, &prereq_id)?;
                }
            }
        }

        Ok(graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MockPlan {
        build_set: HashSet<String>,
        deps_map: HashMap<String, Vec<String>>,
    }

    impl PackageGraphPlan for MockPlan {
        fn build_set(&self) -> &HashSet<String> {
            &self.build_set
        }

        fn deps_for_task(&self, task: &str) -> &[String] {
            self.deps_map.get(task).map(|v| v.as_slice()).unwrap_or(&[])
        }

        fn is_post_bootstrap_full(&self, _task: &str) -> bool {
            false
        }
    }

    #[test]
    fn planner_creates_point_to_point_edges() {
        let mut build_set = HashSet::new();
        let mut deps_map = HashMap::new();

        build_set.insert("zlib".to_string());
        build_set.insert("curl".to_string());

        deps_map.insert("zlib".to_string(), vec![]);
        deps_map.insert("curl".to_string(), vec!["zlib".to_string()]);

        let plan = MockPlan {
            build_set,
            deps_map,
        };

        let graph = ActionPlanner::plan(&plan, &PlannerOptions::default()).unwrap();
        assert_eq!(graph.len(), 10); // 5 actions per package * 2 packages

        let curl_build = ActionId::build("curl");
        let zlib_deploy = ActionId::deploy("zlib");

        let prereqs = graph.dependencies_of(&curl_build).unwrap();
        assert!(prereqs.contains(&zlib_deploy));
    }
}

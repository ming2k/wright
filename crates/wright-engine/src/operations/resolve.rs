use std::collections::HashSet;

use crate::config::GlobalConfig;
use crate::error::Result;
use crate::resolve::{
    BuildExecutionPlan, BuildPlanOptions, DepDomain, MatchPolicy, ResolveOptions,
    create_execution_plan, resolve_build_set,
};

pub struct ResolveRequest {
    pub targets: Vec<String>,
    pub deps: DepDomain,
    pub rdeps: DepDomain,
    pub match_policies: Vec<MatchPolicy>,
    pub depth: Option<usize>,
    pub tree: bool,
}

pub async fn execute_resolve(request: ResolveRequest, config: &GlobalConfig) -> Result<()> {
    let ResolveRequest {
        targets,
        deps,
        rdeps,
        match_policies,
        depth,
        tree,
    } = request;
    let graph_domain = deps | rdeps;
    let mut resolved = resolve_build_set(
        config,
        targets,
        ResolveOptions {
            deps,
            rdeps,
            match_policies,
            depth,
            include_targets: true,
            preserve_targets: true,
        },
    )
    .await?
    .names;
    resolved.sort();

    if !tree {
        for name in resolved {
            crate::outln!("{name}");
        }
        return Ok(());
    }

    let graph = create_execution_plan(
        config,
        resolved,
        &BuildPlanOptions {
            fetch_only: true,
            ..Default::default()
        },
        if graph_domain.is_empty() {
            DepDomain::ALL
        } else {
            graph_domain
        },
    )?;
    for line in render_dependency_forest(&graph) {
        crate::outln!("{line}");
    }
    Ok(())
}

fn render_dependency_forest(graph: &BuildExecutionPlan) -> Vec<String> {
    let mut referenced = HashSet::new();
    for name in graph.build_set() {
        referenced.extend(graph.deps_for_task(name).iter().cloned());
    }

    let mut roots: Vec<&String> = graph
        .build_set()
        .iter()
        .filter(|name| !referenced.contains(*name))
        .collect();
    if roots.is_empty() {
        roots.extend(graph.build_set());
    }
    roots.sort();

    let mut lines = Vec::new();
    let mut rendered = HashSet::new();
    for root in roots {
        render_node(graph, root, "", true, true, &mut rendered, &mut lines);
    }
    lines
}

fn render_node(
    graph: &BuildExecutionPlan,
    name: &str,
    prefix: &str,
    last: bool,
    root: bool,
    rendered: &mut HashSet<String>,
    lines: &mut Vec<String>,
) {
    let branch = if root {
        ""
    } else if last {
        "└── "
    } else {
        "├── "
    };
    let repeated = !rendered.insert(name.to_string());
    lines.push(format!(
        "{prefix}{branch}{name}{}",
        if repeated { " (*)" } else { "" }
    ));
    if repeated {
        return;
    }

    let mut dependencies: Vec<&String> = graph.deps_for_task(name).iter().collect();
    dependencies.sort();
    dependencies.dedup();
    let child_prefix = if root {
        String::new()
    } else if last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };
    for (index, dependency) in dependencies.iter().enumerate() {
        render_node(
            graph,
            dependency,
            &child_prefix,
            index + 1 == dependencies.len(),
            false,
            rendered,
            lines,
        );
    }
}

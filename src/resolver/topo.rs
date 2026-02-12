use std::collections::{HashSet};
use crate::database::PackageInfo;
use crate::error::{WrightError, Result};
use super::graph::DependencyGraph;

pub fn sort_dependencies(graph: &DependencyGraph) -> Result<Vec<PackageInfo>> {
    let mut sorted = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    for name in graph.nodes.keys() {
        visit(name, graph, &mut visited, &mut visiting, &mut sorted)?;
    }

    Ok(sorted)
}

fn visit(
    name: &str,
    graph: &DependencyGraph,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    sorted: &mut Vec<PackageInfo>,
) -> Result<()> {
    if visited.contains(name) {
        return Ok(());
    }
    if visiting.contains(name) {
        return Err(WrightError::DependencyError(format!("circular dependency detected: {}", name)));
    }

    visiting.insert(name.to_string());

    if let Some(node) = graph.nodes.get(name) {
        for dep in &node.dependencies {
            visit(dep, graph, visited, visiting, sorted)?;
        }
        sorted.push(node.info.clone());
    }

    visiting.remove(name);
    visited.insert(name.to_string());

    Ok(())
}

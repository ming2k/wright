use std::collections::{HashMap, HashSet};
use crate::error::{Result, WrightError};
use crate::part::version;
use crate::archive::resolver::ResolvedPart;

pub fn sort_dependencies(
    resolved_map: &HashMap<String, ResolvedPart>,
) -> Result<Vec<String>> {
    let mut sorted_names = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    for name in resolved_map.keys() {
        visit_resolved(
            name,
            resolved_map,
            &mut visited,
            &mut visiting,
            &mut sorted_names,
        )?;
    }

    Ok(sorted_names)
}

fn visit_resolved(
    name: &str,
    map: &HashMap<String, ResolvedPart>,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    sorted: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(name) {
        return Ok(());
    }
    if visiting.contains(name) {
        return Err(WrightError::DependencyError(format!(
            "circular dependency: {}",
            name
        )));
    }

    visiting.insert(name.to_string());

    if let Some(pkg) = map.get(name) {
        for dep in &pkg.dependencies {
            let (dep_name, _) =
                version::parse_dependency(dep).unwrap_or_else(|_| (dep.clone(), None));
            if map.contains_key(&dep_name) {
                visit_resolved(&dep_name, map, visited, visiting, sorted)?;
            }
        }
    }

    visiting.remove(name);
    visited.insert(name.to_string());
    sorted.push(name.to_string());

    Ok(())
}

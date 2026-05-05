use crate::error::{Result, WrightError};
use crate::part::store::ResolvedPart;
use crate::part::version;
use std::collections::{HashMap, HashSet};

pub fn sort_dependencies(resolved_map: &HashMap<String, ResolvedPart>) -> Result<Vec<String>> {
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

    if let Some(part) = map.get(name) {
        for dep in &part.dependencies {
            let (dep_name, _) =
                version::parse_dependency(dep).unwrap_or_else(|_| (dep.clone(), None));
            let (_, output_name) = version::parse_dep_ref(&dep_name);
            if map.contains_key(&output_name) {
                visit_resolved(&output_name, map, visited, visiting, sorted)?;
            }
        }
    }

    visiting.remove(name);
    visited.insert(name.to_string());
    sorted.push(name.to_string());

    Ok(())
}

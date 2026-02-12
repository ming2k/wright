use std::collections::{HashMap, HashSet};
use crate::database::{Database, PackageInfo};
use crate::error::{WrightError, Result};

pub struct DependencyGraph {
    pub nodes: HashMap<String, PackageNode>,
}

pub struct PackageNode {
    pub info: PackageInfo,
    pub dependencies: Vec<String>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    /// Recursively resolve dependencies for a list of packages.
    pub fn resolve(&mut self, db: &Database, targets: &[String]) -> Result<()> {
        let mut queue: Vec<String> = targets.iter().cloned().collect();
        let mut visited = HashSet::new();

        while let Some(name) = queue.pop() {
            if visited.contains(&name) {
                continue;
            }

            let pkg_info = db.get_package(&name)?
                .ok_or_else(|| WrightError::DependencyError(format!("package not found in database: {}", name)))?;
            
            let pkg_id = pkg_info.id;
            let deps = db.get_dependencies(pkg_id)?;
            
            let dep_names: Vec<String> = deps.iter().map(|(name, _): &(String, Option<String>)| name.clone()).collect();
            
            for dep_name in &dep_names {
                if !visited.contains(dep_name) {
                    queue.push(dep_name.clone());
                }
            }

            self.nodes.insert(name.clone(), PackageNode {
                info: pkg_info,
                dependencies: dep_names,
            });
            
            visited.insert(name);
        }

        Ok(())
    }
}

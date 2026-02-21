//! Query and analysis operations — dependency tree rendering, etc.

use crate::error::{Result, WrightResultExt};
use rusqlite::params;

use crate::database::{Database, DepType};

/// Render the forward dependency tree for a package into a writer.
pub fn write_dep_tree(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    filter: Option<&str>,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let mut visited = std::collections::HashSet::new();
    let mut ancestors = std::collections::HashSet::new();
    visited.insert(name.to_string());
    ancestors.insert(name.to_string());
    write_dep_tree_inner(db, name, prefix, current_depth, max_depth, filter, &mut visited, &mut ancestors, out)
}

fn write_dep_tree_inner(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    filter: Option<&str>,
    visited: &mut std::collections::HashSet<String>,
    ancestors: &mut std::collections::HashSet<String>,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    if current_depth > max_depth {
        return Ok(());
    }

    let deps = db.get_dependencies_by_name(name)
        .context(format!("failed to get dependencies for {}", name))?;

    let children: Vec<_> = if let Some(f) = filter {
        deps.iter().filter(|d| d.name.contains(f)).collect()
    } else {
        deps.iter().collect()
    };

    for (i, dep) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;
        let connector = if is_last_child { "└── " } else { "├── " };
        let version_info = dep.constraint.as_ref()
            .map(|c| format!(" ({})", c))
            .unwrap_or_default();
        let type_info = if dep.dep_type == DepType::Link { " [link]" } else { "" };

        if ancestors.contains(&dep.name) {
            // True cycle: this node is our own ancestor in the current path
            writeln!(out, "{}{}{}{}{} (cycle)", prefix, connector, dep.name, version_info, type_info)?;
        } else if visited.contains(&dep.name) {
            // Already fully expanded elsewhere in the tree (diamond dependency)
            let installed = db.get_package(&dep.name).unwrap_or(None).is_some();
            let installed_mark = if installed { "" } else { " [not installed]" };
            writeln!(out, "{}{}{}{}{}{} (*)", prefix, connector, dep.name, version_info, type_info, installed_mark)?;
        } else {
            let installed = db.get_package(&dep.name).unwrap_or(None).is_some();
            let installed_mark = if installed { "" } else { " [not installed]" };
            writeln!(out, "{}{}{}{}{}{}", prefix, connector, dep.name, version_info, type_info, installed_mark)?;

            if installed {
                visited.insert(dep.name.clone());
                ancestors.insert(dep.name.clone());
                let new_prefix = format!("{}{}", prefix,
                    if is_last_child { "    " } else { "│   " });
                write_dep_tree_inner(db, &dep.name, &new_prefix,
                    current_depth + 1, max_depth, filter, visited, ancestors, out)?;
                ancestors.remove(&dep.name);
            }
        }
    }

    Ok(())
}

/// Render the reverse dependency tree for a package into a writer.
pub fn write_reverse_dep_tree(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    filter: Option<&str>,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let mut visited = std::collections::HashSet::new();
    let mut ancestors = std::collections::HashSet::new();
    visited.insert(name.to_string());
    ancestors.insert(name.to_string());
    write_reverse_dep_tree_inner(db, name, prefix, current_depth, max_depth, filter, &mut visited, &mut ancestors, out)
}

fn write_reverse_dep_tree_inner(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    filter: Option<&str>,
    visited: &mut std::collections::HashSet<String>,
    ancestors: &mut std::collections::HashSet<String>,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    if current_depth > max_depth {
        return Ok(());
    }

    let dependents = db.get_dependents(name)
        .context(format!("failed to get dependents of {}", name))?;

    let children: Vec<_> = if let Some(f) = filter {
        dependents.iter().filter(|(n, _)| n.contains(f)).collect()
    } else {
        dependents.iter().collect()
    };

    for (i, (dep_name, dep_type)) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;
        let connector = if is_last_child { "└── " } else { "├── " };
        let type_info = if *dep_type == "link" { " [link]" } else { "" };

        if ancestors.contains(dep_name.as_str()) {
            writeln!(out, "{}{}{}{} (cycle)", prefix, connector, dep_name, type_info)?;
        } else if visited.contains(dep_name.as_str()) {
            writeln!(out, "{}{}{}{} (*)", prefix, connector, dep_name, type_info)?;
        } else {
            writeln!(out, "{}{}{}{}", prefix, connector, dep_name, type_info)?;
            visited.insert(dep_name.clone());
            ancestors.insert(dep_name.clone());
            let new_prefix = format!("{}{}", prefix,
                if is_last_child { "    " } else { "│   " });
            write_reverse_dep_tree_inner(db, dep_name, &new_prefix,
                current_depth + 1, max_depth, filter, visited, ancestors, out)?;
            ancestors.remove(dep_name.as_str());
        }
    }

    Ok(())
}

/// Render the full system dependency tree into a writer.
pub fn write_system_tree(db: &Database, out: &mut dyn std::io::Write) -> Result<()> {
    let roots = db.get_root_packages()?;
    if roots.is_empty() {
        let all = db.list_packages()?;
        if all.is_empty() {
            writeln!(out, "No packages installed.")?;
        } else {
            writeln!(out, "No root packages found; the system may have circular dependencies.")?;
        }
        return Ok(());
    }

    let mut visited = std::collections::HashSet::new();
    for (i, root) in roots.iter().enumerate() {
        writeln!(out, "{}", root.name)?;
        let mut ancestors = std::collections::HashSet::new();
        visited.insert(root.name.clone());
        ancestors.insert(root.name.clone());
        write_dep_tree_inner(db, &root.name, "", 1, usize::MAX, None, &mut visited, &mut ancestors, out)?;
        if i < roots.len() - 1 {
            writeln!(out)?;
        }
    }

    Ok(())
}

/// Check all installed packages for broken dependencies.
pub fn check_dependencies(db: &Database) -> Result<Vec<String>> {
    let all_packages = db.list_packages()?;
    let mut broken = Vec::new();

    for pkg in all_packages {
        let deps = db.get_dependencies(pkg.id)?;
        for dep in deps {
            if db.get_package(&dep.name)?.is_none() {
                let constraint_str = dep.constraint.map(|c| format!(" ({})", c)).unwrap_or_default();
                broken.push(format!(
                    "Package '{}' has a broken dependency: '{}'{} not found",
                    pkg.name, dep.name, constraint_str
                ));
            }
        }
    }

    Ok(broken)
}

/// Check for circular dependencies in the installed database.
pub fn check_circular_dependencies(db: &Database) -> Result<Vec<String>> {
    let all_packages = db.list_packages()?;
    let mut issues = Vec::new();

    for pkg in all_packages {
        if let Err(e) = db.get_recursive_dependents(&pkg.name) {
            if e.to_string().contains("circular") {
                issues.push(format!("Circular dependency detected involving package '{}'", pkg.name));
            }
        }
    }

    Ok(issues)
}

/// Check if multiple packages claim ownership of the same file.
pub fn check_file_ownership_conflicts(db: &Database) -> Result<Vec<String>> {
    let mut stmt = db.connection().prepare(
        "SELECT path, COUNT(package_id) as count FROM files
         WHERE file_type != 'dir'
         GROUP BY path HAVING count > 1"
    )?;

    let rows = stmt
        .query_map([], |row| {
            let path: String = row.get(0)?;
            Ok(path)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut issues = Vec::new();
    for path in rows {
        // Find who the owners are
        let mut owner_stmt = db.connection().prepare(
            "SELECT p.name FROM packages p JOIN files f ON p.id = f.package_id WHERE f.path = ?1"
        )?;
        let owners = owner_stmt
            .query_map(params![path], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        issues.push(format!("File conflict: '{}' is claimed by multiple packages: {}", path, owners.join(", ")));
    }

    Ok(issues)
}

/// Get recorded shadowed file information.
pub fn check_shadowed_files(db: &Database) -> Result<Vec<String>> {
    Ok(db.get_shadowed_conflicts()?)
}

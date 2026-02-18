//! Query and analysis operations — dependency tree rendering, etc.

use anyhow::{Context, Result};
use rusqlite::params;

use crate::database::Database;

/// Print the forward dependency tree for a package.
pub fn print_dep_tree(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    filter: Option<&str>,
) -> Result<()> {
    if current_depth > max_depth {
        return Ok(());
    }

    let deps = db.get_dependencies_by_name(name)
        .context(format!("failed to get dependencies for {}", name))?;

    let children: Vec<_> = if let Some(f) = filter {
        deps.iter().filter(|(n, _, _)| n.contains(f)).collect()
    } else {
        deps.iter().collect()
    };

    for (i, (dep_name, constraint, dep_type)) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;
        let connector = if is_last_child { "└── " } else { "├── " };
        let version_info = constraint.as_ref()
            .map(|c| format!(" ({})", c))
            .unwrap_or_default();
        let type_info = if *dep_type == "link" { " [link]" } else { "" };
        let installed_mark = if db.get_package(dep_name)
            .unwrap_or(None).is_some() { "" } else { " [not installed]" };

        println!("{}{}{}{}{}{}", prefix, connector, dep_name, version_info, type_info, installed_mark);

        if db.get_package(dep_name).unwrap_or(None).is_some() {
            let new_prefix = format!("{}{}", prefix,
                if is_last_child { "    " } else { "│   " });
            print_dep_tree(db, dep_name, &new_prefix,
                current_depth + 1, max_depth, filter)?;
        }
    }

    Ok(())
}

/// Print the reverse dependency tree for a package (what depends on it).
pub fn print_reverse_dep_tree(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    filter: Option<&str>,
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

        println!("{}{}{}{}", prefix, connector, dep_name, type_info);

        let new_prefix = format!("{}{}", prefix,
            if is_last_child { "    " } else { "│   " });
        print_reverse_dep_tree(db, dep_name, &new_prefix,
            current_depth + 1, max_depth, filter)?;
    }

    Ok(())
}

/// Print the full system dependency tree starting from root packages.
pub fn print_system_tree(db: &Database) -> Result<()> {
    let roots = db.get_root_packages()?;
    if roots.is_empty() {
        let all = db.list_packages()?;
        if all.is_empty() {
            println!("No packages installed.");
        } else {
            println!("Circular dependencies detected or system in inconsistent state (no root packages).");
        }
        return Ok(());
    }

    for (i, root) in roots.iter().enumerate() {
        println!("{}", root.name);
        print_dep_tree(db, &root.name, "", 1, usize::MAX, None)?;
        if i < roots.len() - 1 {
            println!();
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
        for (dep_name, constraint, _) in deps {
            if db.get_package(&dep_name)?.is_none() {
                let constraint_str = constraint.map(|c| format!(" ({})", c)).unwrap_or_default();
                broken.push(format!(
                    "Package '{}' has a broken dependency: '{}'{} not found",
                    pkg.name, dep_name, constraint_str
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

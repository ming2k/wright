//! Query and analysis operations — dependency tree rendering, etc.

use anyhow::{Context, Result};

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
        deps.iter().filter(|(n, _)| n.contains(f)).collect()
    } else {
        deps.iter().collect()
    };

    for (i, (dep_name, constraint)) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;
        let connector = if is_last_child { "└── " } else { "├── " };
        let version_info = constraint.as_ref()
            .map(|c| format!(" ({})", c))
            .unwrap_or_default();
        let installed_mark = if db.get_package(dep_name)
            .unwrap_or(None).is_some() { "" } else { " [not installed]" };

        println!("{}{}{}{}{}", prefix, connector, dep_name, version_info, installed_mark);

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
        dependents.iter().filter(|n| n.contains(f)).collect()
    } else {
        dependents.iter().collect()
    };

    for (i, dep_name) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;
        let connector = if is_last_child { "└── " } else { "├── " };

        println!("{}{}{}", prefix, connector, dep_name);

        let new_prefix = format!("{}{}", prefix,
            if is_last_child { "    " } else { "│   " });
        print_reverse_dep_tree(db, dep_name, &new_prefix,
            current_depth + 1, max_depth, filter)?;
    }

    Ok(())
}

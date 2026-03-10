//! Query and analysis operations — dependency tree rendering, etc.

use crate::error::{Result, WrightResultExt};
use rusqlite::params;

use crate::database::{Database, DepType};

use owo_colors::OwoColorize;

/// How the tree prefix is rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixMode {
    /// Classic tree-drawing characters (├──, └──, │)
    Indent,
    /// Flat list with a depth number prefix
    Depth,
    /// Bare package names (deduplicated, no tree chrome)
    None,
}

/// Options that control tree rendering.
pub struct TreeOptions<'a> {
    pub max_depth: usize,
    pub filter: Option<&'a str>,
    pub prefix_mode: PrefixMode,
    pub prune: &'a [String],
    pub color: bool,
}

/// Accumulated statistics from a tree walk.
#[derive(Default)]
pub struct TreeStats {
    pub total: usize,
    pub max_depth_seen: usize,
    pub not_installed: usize,
    pub cycles: usize,
}

impl TreeStats {
    pub fn write_summary(&self, out: &mut dyn std::io::Write, color: bool) -> std::io::Result<()> {
        let line = format!(
            "{} packages, max depth {}, {} not installed, {} cycles",
            self.total, self.max_depth_seen, self.not_installed, self.cycles,
        );
        if color {
            writeln!(out, "\n{}", line.dimmed())
        } else {
            writeln!(out, "\n{}", line)
        }
    }
}

// ─── helpers for colored output ──────────────────────────────────────────────

fn write_connector(out: &mut dyn std::io::Write, s: &str, color: bool) -> std::io::Result<()> {
    if color {
        write!(out, "{}", s.dimmed())
    } else {
        write!(out, "{}", s)
    }
}

fn write_pkg_name(out: &mut dyn std::io::Write, name: &str, _color: bool) -> std::io::Result<()> {
    write!(out, "{}", name)
}

fn write_version_constraint(
    out: &mut dyn std::io::Write,
    c: &str,
    color: bool,
) -> std::io::Result<()> {
    if color {
        write!(out, " {}", format!("({})", c).green())
    } else {
        write!(out, " ({})", c)
    }
}

fn write_link_tag(out: &mut dyn std::io::Write, color: bool) -> std::io::Result<()> {
    if color {
        write!(out, " {}", "[link]".blue())
    } else {
        write!(out, " [link]")
    }
}

fn write_not_installed(out: &mut dyn std::io::Write, color: bool) -> std::io::Result<()> {
    if color {
        write!(out, " {}", "[not installed]".red())
    } else {
        write!(out, " [not installed]")
    }
}

fn write_cycle_tag(out: &mut dyn std::io::Write, color: bool) -> std::io::Result<()> {
    if color {
        write!(out, " {}", "(cycle)".red().bold())
    } else {
        write!(out, " (cycle)")
    }
}

fn write_dup_tag(out: &mut dyn std::io::Write, color: bool) -> std::io::Result<()> {
    if color {
        write!(out, " {}", "(*)".dimmed())
    } else {
        write!(out, " (*)")
    }
}

fn write_pruned_tag(out: &mut dyn std::io::Write, color: bool) -> std::io::Result<()> {
    if color {
        write!(out, " {}", "(pruned)".yellow())
    } else {
        write!(out, " (pruned)")
    }
}

// ─── forward dependency tree ─────────────────────────────────────────────────

/// Render the forward dependency tree for a package into a writer.
pub fn write_dep_tree(
    db: &Database,
    name: &str,
    opts: &TreeOptions,
    out: &mut dyn std::io::Write,
) -> Result<TreeStats> {
    let mut visited = std::collections::HashSet::new();
    let mut ancestors = std::collections::HashSet::new();
    let mut stats = TreeStats::default();
    stats.total = 1; // root package
    visited.insert(name.to_string());
    ancestors.insert(name.to_string());
    write_dep_tree_inner(
        db,
        name,
        "",
        1,
        opts,
        &mut visited,
        &mut ancestors,
        &mut stats,
        out,
    )?;
    Ok(stats)
}

fn write_dep_tree_inner(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    opts: &TreeOptions,
    visited: &mut std::collections::HashSet<String>,
    ancestors: &mut std::collections::HashSet<String>,
    stats: &mut TreeStats,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    if current_depth > opts.max_depth {
        return Ok(());
    }

    let deps = db
        .get_dependencies_by_name(name)
        .context(format!("failed to get dependencies for {}", name))?;

    let children: Vec<_> = if let Some(f) = opts.filter {
        deps.iter().filter(|d| d.name.contains(f)).collect()
    } else {
        deps.iter().collect()
    };

    // For PrefixMode::None, we need a set to deduplicate
    let seen_none: Option<&std::collections::HashSet<String>> =
        if opts.prefix_mode == PrefixMode::None {
            Some(visited)
        } else {
            Option::None
        };
    let _ = seen_none; // used below

    for (i, dep) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;

        // Check prune
        if opts.prune.contains(&dep.name) {
            stats.total += 1;
            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, &dep.name, opts.color)?;
            if let Some(c) = &dep.constraint {
                write_version_constraint(out, c, opts.color)?;
            }
            if dep.dep_type == DepType::Link {
                write_link_tag(out, opts.color)?;
            }
            write_pruned_tag(out, opts.color)?;
            writeln!(out)?;
            continue;
        }

        if ancestors.contains(&dep.name) {
            // True cycle
            stats.total += 1;
            stats.cycles += 1;
            if current_depth > stats.max_depth_seen {
                stats.max_depth_seen = current_depth;
            }
            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, &dep.name, opts.color)?;
            if let Some(c) = &dep.constraint {
                write_version_constraint(out, c, opts.color)?;
            }
            if dep.dep_type == DepType::Link {
                write_link_tag(out, opts.color)?;
            }
            write_cycle_tag(out, opts.color)?;
            writeln!(out)?;
        } else if visited.contains(&dep.name) {
            // Already fully expanded elsewhere (diamond)
            if opts.prefix_mode == PrefixMode::None {
                // skip duplicates in None mode
                continue;
            }
            stats.total += 1;
            if current_depth > stats.max_depth_seen {
                stats.max_depth_seen = current_depth;
            }
            let installed = db.get_package(&dep.name).unwrap_or(None).is_some();
            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, &dep.name, opts.color)?;
            if let Some(c) = &dep.constraint {
                write_version_constraint(out, c, opts.color)?;
            }
            if dep.dep_type == DepType::Link {
                write_link_tag(out, opts.color)?;
            }
            if !installed {
                write_not_installed(out, opts.color)?;
                stats.not_installed += 1;
            }
            write_dup_tag(out, opts.color)?;
            writeln!(out)?;
        } else {
            let installed = db.get_package(&dep.name).unwrap_or(None).is_some();
            stats.total += 1;
            if current_depth > stats.max_depth_seen {
                stats.max_depth_seen = current_depth;
            }

            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, &dep.name, opts.color)?;
            if let Some(c) = &dep.constraint {
                write_version_constraint(out, c, opts.color)?;
            }
            if dep.dep_type == DepType::Link {
                write_link_tag(out, opts.color)?;
            }
            if !installed {
                write_not_installed(out, opts.color)?;
                stats.not_installed += 1;
            }
            writeln!(out)?;

            if installed {
                visited.insert(dep.name.clone());
                ancestors.insert(dep.name.clone());
                let new_prefix = match opts.prefix_mode {
                    PrefixMode::Indent => {
                        format!("{}{}", prefix, if is_last_child { "    " } else { "│   " })
                    }
                    _ => String::new(),
                };
                write_dep_tree_inner(
                    db,
                    &dep.name,
                    &new_prefix,
                    current_depth + 1,
                    opts,
                    visited,
                    ancestors,
                    stats,
                    out,
                )?;
                ancestors.remove(&dep.name);
            }
        }
    }

    Ok(())
}

/// Write the line prefix according to the chosen mode.
fn write_line_prefix(
    out: &mut dyn std::io::Write,
    opts: &TreeOptions,
    prefix: &str,
    is_last_child: bool,
    depth: usize,
) -> std::io::Result<()> {
    match opts.prefix_mode {
        PrefixMode::Indent => {
            let connector = if is_last_child {
                "└── "
            } else {
                "├── "
            };
            write_connector(out, prefix, opts.color)?;
            write_connector(out, connector, opts.color)?;
        }
        PrefixMode::Depth => {
            let tag = format!("{} ", depth);
            write_connector(out, &tag, opts.color)?;
        }
        PrefixMode::None => { /* no prefix */ }
    }
    Ok(())
}

// ─── reverse dependency tree ─────────────────────────────────────────────────

/// Render the reverse dependency tree for a package into a writer.
pub fn write_reverse_dep_tree(
    db: &Database,
    name: &str,
    opts: &TreeOptions,
    out: &mut dyn std::io::Write,
) -> Result<TreeStats> {
    let mut visited = std::collections::HashSet::new();
    let mut ancestors = std::collections::HashSet::new();
    let mut stats = TreeStats::default();
    stats.total = 1;
    visited.insert(name.to_string());
    ancestors.insert(name.to_string());
    write_reverse_dep_tree_inner(
        db,
        name,
        "",
        1,
        opts,
        &mut visited,
        &mut ancestors,
        &mut stats,
        out,
    )?;
    Ok(stats)
}

fn write_reverse_dep_tree_inner(
    db: &Database,
    name: &str,
    prefix: &str,
    current_depth: usize,
    opts: &TreeOptions,
    visited: &mut std::collections::HashSet<String>,
    ancestors: &mut std::collections::HashSet<String>,
    stats: &mut TreeStats,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    if current_depth > opts.max_depth {
        return Ok(());
    }

    let dependents = db
        .get_dependents(name)
        .context(format!("failed to get dependents of {}", name))?;

    let children: Vec<_> = if let Some(f) = opts.filter {
        dependents.iter().filter(|(n, _)| n.contains(f)).collect()
    } else {
        dependents.iter().collect()
    };

    for (i, (dep_name, dep_type)) in children.iter().enumerate() {
        let is_last_child = i == children.len() - 1;
        let is_link = *dep_type == "link";

        // Check prune
        if opts.prune.iter().any(|p| p == dep_name) {
            stats.total += 1;
            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, dep_name, opts.color)?;
            if is_link {
                write_link_tag(out, opts.color)?;
            }
            write_pruned_tag(out, opts.color)?;
            writeln!(out)?;
            continue;
        }

        if ancestors.contains(dep_name.as_str()) {
            stats.total += 1;
            stats.cycles += 1;
            if current_depth > stats.max_depth_seen {
                stats.max_depth_seen = current_depth;
            }
            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, dep_name, opts.color)?;
            if is_link {
                write_link_tag(out, opts.color)?;
            }
            write_cycle_tag(out, opts.color)?;
            writeln!(out)?;
        } else if visited.contains(dep_name.as_str()) {
            if opts.prefix_mode == PrefixMode::None {
                continue;
            }
            stats.total += 1;
            if current_depth > stats.max_depth_seen {
                stats.max_depth_seen = current_depth;
            }
            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, dep_name, opts.color)?;
            if is_link {
                write_link_tag(out, opts.color)?;
            }
            write_dup_tag(out, opts.color)?;
            writeln!(out)?;
        } else {
            stats.total += 1;
            if current_depth > stats.max_depth_seen {
                stats.max_depth_seen = current_depth;
            }
            write_line_prefix(out, opts, prefix, is_last_child, current_depth)?;
            write_pkg_name(out, dep_name, opts.color)?;
            if is_link {
                write_link_tag(out, opts.color)?;
            }
            writeln!(out)?;

            visited.insert(dep_name.clone());
            ancestors.insert(dep_name.clone());
            let new_prefix = match opts.prefix_mode {
                PrefixMode::Indent => {
                    format!("{}{}", prefix, if is_last_child { "    " } else { "│   " })
                }
                _ => String::new(),
            };
            write_reverse_dep_tree_inner(
                db,
                dep_name,
                &new_prefix,
                current_depth + 1,
                opts,
                visited,
                ancestors,
                stats,
                out,
            )?;
            ancestors.remove(dep_name.as_str());
        }
    }

    Ok(())
}

// ─── system tree ─────────────────────────────────────────────────────────────

/// Render the full system dependency tree into a writer.
pub fn write_system_tree(
    db: &Database,
    opts: &TreeOptions,
    out: &mut dyn std::io::Write,
) -> Result<TreeStats> {
    let roots = db.get_root_packages()?;
    if roots.is_empty() {
        let all = db.list_packages()?;
        if all.is_empty() {
            writeln!(out, "No packages installed.")?;
        } else {
            writeln!(
                out,
                "No root packages found; the system may have circular dependencies."
            )?;
        }
        return Ok(TreeStats::default());
    }

    let mut visited = std::collections::HashSet::new();
    let mut combined_stats = TreeStats::default();

    for (i, root) in roots.iter().enumerate() {
        writeln!(out, "{}", root.name)?;
        combined_stats.total += 1;
        let mut ancestors = std::collections::HashSet::new();
        visited.insert(root.name.clone());
        ancestors.insert(root.name.clone());

        let sys_opts = TreeOptions {
            max_depth: opts.max_depth,
            filter: opts.filter,
            prefix_mode: opts.prefix_mode,
            prune: opts.prune,
            color: opts.color,
        };
        write_dep_tree_inner(
            db,
            &root.name,
            "",
            1,
            &sys_opts,
            &mut visited,
            &mut ancestors,
            &mut combined_stats,
            out,
        )?;
        if i < roots.len() - 1 {
            writeln!(out)?;
        }
    }

    Ok(combined_stats)
}

// ─── health-check functions (unchanged) ──────────────────────────────────────

/// Check all installed packages for broken dependencies.
pub fn check_dependencies(db: &Database) -> Result<Vec<String>> {
    let all_packages = db.list_packages()?;
    let mut broken = Vec::new();

    for pkg in all_packages {
        let deps = db.get_dependencies(pkg.id)?;
        for dep in deps {
            if db.get_package(&dep.name)?.is_none() && db.find_providers(&dep.name)?.is_empty() {
                let constraint_str = dep
                    .constraint
                    .map(|c| format!(" ({})", c))
                    .unwrap_or_default();
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
                issues.push(format!(
                    "Circular dependency detected involving package '{}'",
                    pkg.name
                ));
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
         GROUP BY path HAVING count > 1",
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
            "SELECT p.name FROM packages p JOIN files f ON p.id = f.package_id WHERE f.path = ?1",
        )?;
        let owners = owner_stmt
            .query_map(params![path], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        issues.push(format!(
            "File conflict: '{}' is claimed by multiple packages: {}",
            path,
            owners.join(", ")
        ));
    }

    Ok(issues)
}

/// Get recorded shadowed file information.
pub fn check_shadowed_files(db: &Database) -> Result<Vec<String>> {
    Ok(db.get_shadowed_conflicts()?)
}

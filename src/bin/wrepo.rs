use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wright::config::GlobalConfig;
use wright::repo;

#[derive(Parser)]
#[command(name = "wrepo", about = "wright repository manager")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to config file
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Increase verbosity (-v or -vv)
    #[arg(long, short, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Suppress non-error output
    #[arg(long, short, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan a directory and generate/update wright.index.toml
    Sync {
        /// Directory containing .wright.tar.zst files (default: components_dir)
        dir: Option<PathBuf>,
    },
    /// List parts available in indexed repositories
    List {
        /// Show all versions of a specific part
        name: Option<String>,
    },
    /// Search available parts by keyword
    Search {
        /// Search keyword (matches name and description)
        keyword: String,
    },
    /// Remove a part entry from the repository index
    Remove {
        /// Part name
        name: String,
        /// Part version (e.g. "1.2.3" or "1.2.3-2" for specific release)
        version: String,
        /// Also delete the archive file from disk
        #[arg(long)]
        purge: bool,
    },
    /// Manage repository sources
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },
}

#[derive(Subcommand)]
enum SourceAction {
    /// Add a new repository source
    Add {
        /// Unique source name
        name: String,

        /// Source type: local or hold
        #[arg(long, default_value = "local")]
        r#type: String,

        /// Local directory path
        #[arg(long)]
        path: PathBuf,

        /// Priority (higher = preferred)
        #[arg(long, default_value = "100")]
        priority: i32,
    },
    /// Remove a repository source
    Remove {
        /// Source name to remove
        name: String,
    },
    /// List configured repository sources
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose > 1 {
        EnvFilter::new("trace")
    } else if cli.verbose > 0 {
        EnvFilter::new("debug")
    } else if cli.quiet {
        EnvFilter::new("warn")
    } else {
        EnvFilter::new("info")
    };

    if cli.verbose > 0 {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .init();
    }

    let config = GlobalConfig::load(cli.config.as_deref()).context("failed to load config")?;

    let repo_config =
        wright::config::RepoConfig::load(None).context("failed to load repo config")?;

    let mut resolver =
        wright::repo::source::SimpleResolver::new(config.general.cache_dir.join("packages"));
    resolver.load_from_config(&repo_config);
    resolver.add_search_dir(config.general.cache_dir.join("packages"));
    resolver.add_search_dir(config.general.components_dir.clone());

    let db_path = config.general.db_path.clone();
    let db = wright::database::Database::open(&db_path).context("failed to open database")?;

    match cli.command {
        Commands::Sync { dir } => {
            let dir = dir.unwrap_or_else(|| config.general.components_dir.clone());
            if !dir.exists() {
                eprintln!("error: directory '{}' does not exist", dir.display());
                std::process::exit(1);
            }
            let index = repo::index::generate_index(&dir)
                .context("failed to generate index")?;
            repo::index::write_index(&index, &dir)
                .context("failed to write index")?;
            println!(
                "indexed {} part(s) in {}",
                index.parts.len(),
                dir.display()
            );
        }
        Commands::List { name } => {
            use wright::part::version::Version;

            let mut found = false;
            for dir in &resolver.search_dirs {
                if let Some(index) =
                    repo::index::read_index(dir).context("failed to read index")?
                {
                    let mut entries: Vec<_> = if let Some(ref n) = name {
                        index.parts.iter().filter(|e| e.name == *n).collect()
                    } else {
                        index.parts.iter().collect()
                    };

                    entries.sort_by(|a, b| {
                        a.name.cmp(&b.name).then_with(|| {
                            let av = Version::parse(&a.version).ok();
                            let bv = Version::parse(&b.version).ok();
                            match (bv, av) {
                                (Some(bv), Some(av)) => {
                                    b.epoch.cmp(&a.epoch)
                                        .then_with(|| bv.cmp(&av))
                                        .then_with(|| b.release.cmp(&a.release))
                                }
                                _ => b.version.cmp(&a.version)
                                    .then_with(|| b.release.cmp(&a.release)),
                            }
                        })
                    });

                    for entry in &entries {
                        let installed = db.get_package(&entry.name).ok().flatten();
                        let tag = match &installed {
                            Some(pkg)
                                if pkg.version == entry.version
                                    && pkg.release == entry.release =>
                            {
                                " [installed]"
                            }
                            _ => "",
                        };
                        println!(
                            "{} {}-{} ({}){}",
                            entry.name,
                            entry.version,
                            entry.release,
                            entry.arch,
                            tag
                        );
                        found = true;
                    }
                }
            }
            if !found {
                if let Some(n) = name {
                    println!("no parts found for '{}'", n);
                } else {
                    println!("no parts in any repository (run 'wrepo sync' first)");
                }
            }
        }
        Commands::Search { keyword } => {
            let mut found = false;
            for dir in &resolver.search_dirs {
                if let Some(index) =
                    repo::index::read_index(dir).context("failed to read repo index")?
                {
                    for entry in &index.parts {
                        if entry.name.contains(&keyword)
                            || entry
                                .description
                                .to_lowercase()
                                .contains(&keyword.to_lowercase())
                        {
                            let installed = db.get_package(&entry.name).ok().flatten();
                            let tag = if installed.is_some() {
                                " [installed]"
                            } else {
                                ""
                            };
                            println!(
                                "{} {}-{} - {}{}",
                                entry.name,
                                entry.version,
                                entry.release,
                                entry.description,
                                tag
                            );
                            found = true;
                        }
                    }
                }
            }
            if !found {
                println!(
                    "no available packages found matching '{}' (run 'wrepo sync' first?)",
                    keyword
                );
            }
        }
        Commands::Remove {
            name,
            version,
            purge,
        } => {
            let (target_ver, target_rel) = if let Some(pos) = version.rfind('-') {
                if let Ok(rel) = version[pos + 1..].parse::<u32>() {
                    (&version[..pos], Some(rel))
                } else {
                    (version.as_str(), None)
                }
            } else {
                (version.as_str(), None)
            };

            let mut removed = false;
            for dir in &resolver.search_dirs {
                if let Some(index) = repo::index::read_index(dir)? {
                    let (to_remove, remaining): (Vec<_>, Vec<_>) =
                        index.parts.into_iter().partition(|e| {
                            e.name == name
                                && e.version == target_ver
                                && target_rel.map_or(true, |r| e.release == r)
                        });

                    if to_remove.is_empty() {
                        continue;
                    }

                    let updated = repo::index::RepoIndex { parts: remaining };
                    repo::index::write_index(&updated, dir)
                        .context("failed to write index")?;

                    for entry in &to_remove {
                        println!("removed: {} {}-{}", entry.name, entry.version, entry.release);
                        if purge {
                            let file_path = dir.join(&entry.filename);
                            if file_path.exists() {
                                std::fs::remove_file(&file_path).context(format!(
                                    "failed to delete {}",
                                    file_path.display()
                                ))?;
                                println!("  deleted: {}", file_path.display());
                            }
                        }
                    }
                    removed = true;
                    break;
                }
            }

            if !removed {
                eprintln!(
                    "error: {} {} not found in any repository index",
                    name, version
                );
                std::process::exit(1);
            }
        }
        Commands::Source { action } => {
            let repos_path = PathBuf::from("/etc/wright/repos.toml");
            match action {
                SourceAction::List => {
                    if !repos_path.exists() {
                        println!("no sources configured ({})", repos_path.display());
                    } else {
                        let rc = wright::config::RepoConfig::load(Some(&repos_path))
                            .context("failed to load repos.toml")?;
                        if rc.source.is_empty() {
                            println!("no sources configured");
                        } else {
                            for s in &rc.source {
                                let enabled = if s.enabled { "" } else { " [disabled]" };
                                let location = s
                                    .path
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .or_else(|| s.url.clone())
                                    .unwrap_or_default();
                                println!(
                                    "{:<15} {:<8} pri={:<4} {}{}",
                                    s.name, s.type_, s.priority, location, enabled
                                );
                            }
                        }
                    }
                }
                SourceAction::Add {
                    name,
                    r#type,
                    path,
                    priority,
                } => {
                    let type_str = r#type;
                    if type_str != "local" && type_str != "hold" {
                        eprintln!("error: type must be 'local' or 'hold'");
                        std::process::exit(1);
                    }
                    if !path.exists() {
                        eprintln!("warning: path '{}' does not exist yet", path.display());
                    }

                    let mut content = if repos_path.exists() {
                        std::fs::read_to_string(&repos_path).context("failed to read repos.toml")?
                    } else {
                        String::new()
                    };

                    if repos_path.exists() {
                        let rc = wright::config::RepoConfig::load(Some(&repos_path))
                            .context("failed to load repos.toml")?;
                        if rc.source.iter().any(|s| s.name == name) {
                            eprintln!("error: source '{}' already exists", name);
                            std::process::exit(1);
                        }
                    }

                    if !content.is_empty() && !content.ends_with('\n') {
                        content.push('\n');
                    }
                    content.push_str(&format!(
                        "\n[[source]]\nname = \"{}\"\ntype = \"{}\"\npath = \"{}\"\npriority = {}\n",
                        name, type_str, path.display(), priority
                    ));

                    // Ensure parent directory exists
                    if let Some(parent) = repos_path.parent() {
                        std::fs::create_dir_all(parent).context("failed to create config directory")?;
                    }
                    std::fs::write(&repos_path, &content).context("failed to write repos.toml")?;
                    println!("added source '{}' -> {}", name, path.display());
                }
                SourceAction::Remove { name } => {
                    if !repos_path.exists() {
                        eprintln!("error: {} does not exist", repos_path.display());
                        std::process::exit(1);
                    }

                    let rc = wright::config::RepoConfig::load(Some(&repos_path))
                        .context("failed to load repos.toml")?;
                    if !rc.source.iter().any(|s| s.name == name) {
                        eprintln!("error: source '{}' not found", name);
                        std::process::exit(1);
                    }

                    let remaining: Vec<_> = rc.source.iter().filter(|s| s.name != name).collect();

                    let mut content = String::new();
                    for s in &remaining {
                        content.push_str("[[source]]\n");
                        content.push_str(&format!("name = \"{}\"\n", s.name));
                        content.push_str(&format!("type = \"{}\"\n", s.type_));
                        if let Some(ref p) = s.path {
                            content.push_str(&format!("path = \"{}\"\n", p.display()));
                        }
                        if let Some(ref u) = s.url {
                            content.push_str(&format!("url = \"{}\"\n", u));
                        }
                        content.push_str(&format!("priority = {}\n", s.priority));
                        if let Some(ref k) = s.gpg_key {
                            content.push_str(&format!("gpg_key = \"{}\"\n", k.display()));
                        }
                        if !s.enabled {
                            content.push_str("enabled = false\n");
                        }
                        content.push('\n');
                    }

                    std::fs::write(&repos_path, &content).context("failed to write repos.toml")?;
                    println!("removed source '{}'", name);
                }
            }
        }
    }

    Ok(())
}

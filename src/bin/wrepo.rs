use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wright::config::GlobalConfig;
use wright::repo;

#[derive(Parser)]
#[command(name = "wrepo", about = "wright repository manager", version)]
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
    /// Import packages from a directory of .wright.tar.zst archives
    Sync {
        /// Directory containing .wright.tar.zst files (default: components_dir)
        dir: Option<PathBuf>,
    },
    /// List parts available in the repository
    List {
        /// Show all versions of a specific part
        name: Option<String>,
    },
    /// Search available parts by keyword
    Search {
        /// Search keyword (matches name and description)
        keyword: String,
    },
    /// Remove a part entry from the repository
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
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let config = GlobalConfig::load(cli.config.as_deref()).context("failed to load config")?;

    match cli.command {
        Commands::Sync { dir } => {
            let dir = dir.unwrap_or_else(|| config.general.components_dir.clone());
            if !dir.exists() {
                eprintln!("error: directory '{}' does not exist", dir.display());
                std::process::exit(1);
            }
            let repo_db = repo::db::RepoDb::open(&config.general.repo_dir)
                .context("failed to open repo database")?;
            let count = repo_db
                .sync_from_archives(&dir)
                .context("failed to sync archives")?;
            println!("indexed {} part(s) from {}", count, dir.display());
        }
        Commands::List { name } => {
            let repo_db = repo::db::RepoDb::open(&config.general.repo_dir)
                .context("failed to open repo database")?;
            let db_path = config.general.db_path.clone();
            let db =
                wright::database::Database::open(&db_path).context("failed to open database")?;

            let entries = repo_db
                .list_packages(name.as_deref())
                .context("failed to list packages")?;

            if entries.is_empty() {
                if let Some(n) = name {
                    println!("no parts found for '{}'", n);
                } else {
                    println!("no parts in repository (run 'wrepo sync' first)");
                }
            } else {
                for entry in &entries {
                    let installed = db.get_package(&entry.name).ok().flatten();
                    let tag = match &installed {
                        Some(pkg)
                            if pkg.version == entry.version && pkg.release == entry.release =>
                        {
                            " [installed]"
                        }
                        _ => "",
                    };
                    println!(
                        "{} {}-{} ({}){}",
                        entry.name, entry.version, entry.release, entry.arch, tag
                    );
                }
            }
        }
        Commands::Search { keyword } => {
            let repo_db = repo::db::RepoDb::open(&config.general.repo_dir)
                .context("failed to open repo database")?;
            let db_path = config.general.db_path.clone();
            let db =
                wright::database::Database::open(&db_path).context("failed to open database")?;

            let entries = repo_db
                .search_packages(&keyword)
                .context("failed to search packages")?;

            if entries.is_empty() {
                println!(
                    "no available packages found matching '{}' (run 'wrepo sync' first?)",
                    keyword
                );
            } else {
                for entry in &entries {
                    let installed = db.get_package(&entry.name).ok().flatten();
                    let tag = if installed.is_some() {
                        " [installed]"
                    } else {
                        ""
                    };
                    println!(
                        "{} {}-{} - {}{}",
                        entry.name, entry.version, entry.release, entry.description, tag
                    );
                }
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

            let repo_db = repo::db::RepoDb::open(&config.general.repo_dir)
                .context("failed to open repo database")?;

            // Get filename before removal if purge is requested
            let filename = if purge {
                repo_db.get_filename(&name, target_ver, target_rel)?
            } else {
                None
            };

            let removed = repo_db
                .remove_package(&name, target_ver, target_rel)
                .context("failed to remove package")?;

            if removed.is_empty() {
                eprintln!("error: {} {} not found in repository", name, version);
                std::process::exit(1);
            }

            for (n, v, r) in &removed {
                println!("removed: {} {}-{}", n, v, r);
            }

            if purge {
                if let Some(fname) = filename {
                    let file_path = config.general.components_dir.join(&fname);
                    if file_path.exists() {
                        std::fs::remove_file(&file_path)
                            .context(format!("failed to delete {}", file_path.display()))?;
                        println!("  deleted: {}", file_path.display());
                    }
                }
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
                    path,
                    priority,
                } => {
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
                        "\n[[source]]\nname = \"{}\"\ntype = \"local\"\npath = \"{}\"\npriority = {}\n",
                        name, path.display(), priority
                    ));

                    if let Some(parent) = repos_path.parent() {
                        std::fs::create_dir_all(parent)
                            .context("failed to create config directory")?;
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

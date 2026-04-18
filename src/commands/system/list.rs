use anyhow::{Context, Result};
use crate::database::InstalledDb;

pub fn execute_list(
    db: &InstalledDb,
    long: bool,
    roots: bool,
    assumed: bool,
    orphans: bool,
) -> Result<()> {
    let parts = if orphans {
        db.get_orphan_parts()
    } else if roots {
        db.get_root_parts()
    } else {
        db.list_parts()
    }
    .context("failed to list parts")?;

    if parts.is_empty() {
        if orphans {
            println!("no orphan parts");
        } else {
            println!("no parts installed");
        }
    } else {
        for pkg in &parts {
            if assumed && !pkg.assumed {
                continue;
            }
            if long {
                if pkg.assumed {
                    println!("{:<12} {:<24} {}", "external", pkg.name, pkg.version);
                } else {
                    println!(
                        "{:<12} {:<24} {}-{} ({})",
                        pkg.origin, pkg.name, pkg.version, pkg.release, pkg.arch
                    );
                }
            } else {
                println!("{}", pkg.name);
            }
        }
    }
    Ok(())
}

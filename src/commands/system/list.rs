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
        for part in &parts {
            if assumed && !part.assumed {
                continue;
            }

            if long {
                if part.assumed {
                    println!("{:<12} {:<24} {}", "external", part.name, part.version);
                } else {
                    println!(
                        "{:<12} {:<24} {}-{}-{}",
                        part.origin, part.name, part.version, part.release, part.arch
                    );
                }
            } else {
                println!("{}", part.name);
            }
        }
    }
    Ok(())
}

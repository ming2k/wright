use crate::database::InstalledDb;
use crate::error::Result;

pub async fn execute_list(
    db: &InstalledDb,
    long: bool,
    roots: bool,
    assumed: bool,
    orphans: bool,
) -> Result<()> {
    let parts = if orphans {
        db.get_orphan_parts().await?
    } else if roots {
        db.get_root_parts().await?
    } else {
        db.list_parts().await?
    };

    if parts.is_empty() {
        if !assumed && !roots && !orphans {
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

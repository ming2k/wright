use crate::database::InstalledDb;
use crate::error::Result;

pub async fn execute_list(
    db: &InstalledDb,
    long: bool,
    roots: bool,
    provided: bool,
    orphans: bool,
) -> Result<()> {
    let parts = if provided {
        db.get_provided_parts().await?
    } else if orphans {
        db.get_orphan_parts().await?
    } else if roots {
        db.get_root_parts().await?
    } else {
        db.list_parts().await?
    };

    if parts.is_empty() {
        if !provided && !roots && !orphans {
            println!("no parts installed");
        }
    } else {
        for part in &parts {
            if long {
                let ver = if part.version.is_empty() {
                    "-"
                } else {
                    &part.version
                };
                if part.origin == crate::database::Origin::External {
                    println!("{:<12} {:<24} {}", "external", part.name, ver);
                } else {
                    let ver_rel_arch = if part.version.is_empty() {
                        format!("{}-{}", part.release, part.arch)
                    } else {
                        format!("{}-{}-{}", ver, part.release, part.arch)
                    };
                    let plan_info = format!("{} {}", part.plan_name, part.version);
                    println!(
                        "{:<12} {:<24} {:<20} {}",
                        part.origin, part.name, ver_rel_arch, plan_info
                    );
                }
            } else {
                println!("{}", part.name);
            }
        }
    }
    Ok(())
}

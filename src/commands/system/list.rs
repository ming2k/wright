use crate::database::InstalledDb;
use crate::error::Result;

pub async fn execute_list(
    db: &InstalledDb,
    long: bool,
    roots: bool,
    assumed: bool,
    orphans: bool,
    plan: Option<&str>,
) -> Result<()> {
    let parts = if let Some(plan_name) = plan {
        db.get_parts_by_plan(plan_name).await?
    } else if assumed {
        db.get_assumed_parts().await?
    } else if orphans {
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
            if long {
                let ver = if part.version.is_empty() { "-" } else { &part.version };
                if part.assumed {
                    println!("{:<12} {:<24} {}", "external", part.name, ver);
                } else {
                    let ver_rel_arch = if part.version.is_empty() {
                        format!("{}-{}", part.release, part.arch)
                    } else {
                        format!("{}-{}-{}", ver, part.release, part.arch)
                    };
                    let plan_info = if part.plan_id > 0 {
                        part.plan_id.to_string()
                    } else {
                        "-".to_string()
                    };
                    println!("{:<12} {:<24} {:<20} {}", part.origin, part.name, ver_rel_arch, plan_info);
                }
            } else {
                println!("{}", part.name);
            }
        }
    }
    Ok(())
}

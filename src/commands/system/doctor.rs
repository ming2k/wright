use crate::database::InstalledDb;
use crate::error::Result;
use owo_colors::OwoColorize;

pub async fn execute_doctor(db: &InstalledDb) -> Result<()> {
    println!("{}", "Wright System Doctor".bold());
    println!("--------------------");

    // 1. Check DB Integrity
    print!("Checking database integrity... ");
    match db.integrity_check().await {
        Ok(issues) if issues.is_empty() => println!("{}", "OK".green()),
        Ok(issues) => {
            println!("{}", "FAILED".red());
            for issue in issues {
                println!("  - {}", issue);
            }
        }
        Err(e) => println!("{} ({})", "ERROR".red(), e),
    }

    // 2. Check for Shadowed File Conflicts
    print!("Checking for file shadowing conflicts... ");
    match db.get_shadowed_conflicts().await {
        Ok(conflicts) if conflicts.is_empty() => println!("{}", "OK".green()),
        Ok(conflicts) => {
            println!("{}", "WARNING".yellow());
            for conflict in conflicts {
                println!("  - {}", conflict);
            }
        }
        Err(e) => println!("{} ({})", "ERROR".red(), e),
    }

    // 3. Check for Orphan Records (files with no owner)
    // TODO: implement more checks

    Ok(())
}

use crate::database::InstalledDb;
use crate::error::Result;

pub async fn execute_unassume(db: &InstalledDb, name: &str) -> Result<()> {
    match db.unassume_part(name).await {
        Ok(()) => println!("unassumed: {}", name),
        Err(e) => {
            tracing::error!("{:#}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

use crate::database::InstalledDb;
use crate::error::Result;

pub async fn execute_history(db: &InstalledDb, part: Option<&str>) -> Result<()> {
    let records = db.get_history(part).await?;
    if records.is_empty() {
        println!("no history records found");
    } else {
        for r in &records {
            let version = match (&r.old_version, &r.new_version) {
                (None, Some(v)) => v.clone(),
                (Some(v), None) => v.clone(),
                (Some(old), Some(new)) => format!("{} -> {}", old, new),
                (None, None) => String::new(),
            };
            let status = if r.status != crate::database::HistoryStatus::Completed {
                format!(" ({})", r.status)
            } else {
                String::new()
            };
            println!(
                "{}  {:<9} {} {}{}",
                r.timestamp.as_deref().unwrap_or_default(),
                r.action,
                r.part_name,
                version,
                status
            );
        }
    }
    Ok(())
}

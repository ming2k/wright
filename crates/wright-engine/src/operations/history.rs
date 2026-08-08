use crate::error::Result;
use wright_state::database::InstalledDb;

pub async fn execute_history(db: &InstalledDb, part: Option<&str>, json: bool) -> Result<()> {
    let records = db.get_history(part).await?;

    if json {
        let out: Vec<serde_json::Value> = records
            .iter()
            .map(|r| {
                serde_json::json!({
                    "timestamp": r.timestamp.as_deref(),
                    "session_id": r.session_id.as_str(),
                    "command": r.command.as_str(),
                    "part": r.part_name.as_str(),
                    "action": r.action.to_string(),
                    "old_version": r.old_version.as_deref(),
                    "new_version": r.new_version.as_deref(),
                    "status": r.status.to_string(),
                })
            })
            .collect();
        return super::print_json(&out);
    }

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
            let status = if r.status != wright_state::database::HistoryStatus::Completed {
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

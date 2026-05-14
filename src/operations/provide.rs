use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use std::io::IsTerminal;

pub async fn execute_provide(
    db: &InstalledDb,
    name: Option<&str>,
    version: Option<&str>,
    file: Option<&std::path::Path>,
) -> Result<()> {
    let mut entries: Vec<(String, String)> = Vec::new();

    if let Some(path) = file {
        let content = std::fs::read_to_string(path).map_err(WrightError::IoError)?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut parts = trimmed.split_whitespace();
            let n = parts.next().ok_or_else(|| {
                WrightError::ForgeError(format!("missing name in line: {}", trimmed))
            })?;
            let v = parts.next().ok_or_else(|| {
                WrightError::ForgeError(format!("missing version in line: {}", trimmed))
            })?;
            entries.push((n.to_string(), v.to_string()));
        }
    } else if let (Some(n), Some(v)) = (name, version) {
        entries.push((n.to_string(), v.to_string()));
    } else if !std::io::stdin().is_terminal() {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            let line = line.map_err(WrightError::IoError)?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut parts = trimmed.split_whitespace();
            let n = parts.next().ok_or_else(|| {
                WrightError::ForgeError(format!("missing name in line: {}", trimmed))
            })?;
            let v = parts.next().ok_or_else(|| {
                WrightError::ForgeError(format!("missing version in line: {}", trimmed))
            })?;
            entries.push((n.to_string(), v.to_string()));
        }
    } else {
        tracing::error!("provide name and version as arguments, use --file, or pipe input");
        std::process::exit(1);
    }

    let count = entries.len();
    for (n, v) in entries {
        crate::cli_action!("Providing", "{} {}", n, v);
        if let Err(e) = db.provide_part(&n, &v).await {
            tracing::error!("providing {}: {:#}", n, e);
            std::process::exit(1);
        }
    }

    crate::cli_action!("Finished", "provide: {} entry(s)", count);
    Ok(())
}

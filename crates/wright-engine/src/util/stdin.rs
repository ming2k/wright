use std::io::BufRead;

use crate::error::{Result, WrightError};

/// Append non-empty trimmed lines from stdin to `args` when stdin is piped.
pub fn collect_stdin_args(mut args: Vec<String>) -> Result<Vec<String>> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        for line in std::io::stdin().lock().lines() {
            let line = line.map_err(WrightError::IoError)?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                args.push(trimmed.to_string());
            }
        }
    }
    Ok(args)
}

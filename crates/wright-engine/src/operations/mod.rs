pub mod build;
pub mod check;
pub mod clean;
pub mod doctor;
pub mod drive;
pub mod files;
pub mod health;
pub mod history;
pub mod install;
pub mod launch;
pub mod lint;
pub mod list;
pub mod merge;
pub mod owner;
pub mod package;
pub mod provide;
pub mod prune;
pub mod remove;
pub mod resolve;
pub mod upgrade;

mod targets;

/// Serialize `value` as pretty JSON to stdout — the shared `--json` output
/// path for query commands.
pub(crate) fn print_json<T: serde::Serialize>(value: &T) -> crate::error::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|e| {
        crate::error::WrightError::ForgeError(format!("serialize json output: {}", e))
    })?;
    println!("{}", text);
    Ok(())
}

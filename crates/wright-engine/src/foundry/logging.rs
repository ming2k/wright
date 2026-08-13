//! Cargo-style verb and message helpers for forge-flow CLI output.
//!
//! These build the *string content* of a CLI line; the Cargo-style alignment
//! and color are applied by [`crate::util::logging::format_action`] (via the
//! `cli_action!` macro and the tracing CLI layer).

use std::path::Path;

pub use crate::util::display::describe_build_capacity;

/// Cargo-style verb for the start of a stage (gerund).
pub fn stage_verb(stage_name: &str) -> &'static str {
    match stage_name {
        "fetch" => "Fetching",
        "verify" => "Verifying",
        "extract" => "Extracting",
        "prepare" => "Preparing",
        "configure" => "Configuring",
        "compile" => "Compiling",
        "check" => "Checking",
        "staging" => "Staging",
        _ => "Running",
    }
}

/// Pull the filename out of a path; fall back to the full string.
pub fn part_filename(part_path: &Path) -> String {
    part_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| part_path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_verb_maps_builtin_stages() {
        assert_eq!(stage_verb("prepare"), "Preparing");
        assert_eq!(stage_verb("compile"), "Compiling");
        assert_eq!(stage_verb("check"), "Checking");
        assert_eq!(stage_verb("staging"), "Staging");
        assert_eq!(stage_verb("fetch"), "Fetching");
        assert_eq!(stage_verb("custom"), "Running");
    }

    #[test]
    fn part_filename_strips_directory() {
        use std::path::Path;
        assert_eq!(
            part_filename(Path::new("/tmp/linux.wright.tar.zst")),
            "linux.wright.tar.zst"
        );
    }
}

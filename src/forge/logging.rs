//! Cargo-style verb and message helpers for forge-flow CLI output.
//!
//! These build the *string content* of a CLI line; the Cargo-style alignment
//! and color are applied by [`crate::util::logging::format_action`] (via the
//! `cli_action!` macro and the tracing CLI layer).

use std::path::Path;

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

pub fn describe_build_capacity(concurrent_tasks: usize, total_cpus: usize) -> String {
    format!(
        "Forge capacity: {} parallel {} on {} {}.",
        concurrent_tasks,
        pluralize(concurrent_tasks, "task", "tasks"),
        total_cpus,
        pluralize(total_cpus, "CPU core", "CPU cores"),
    )
}

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

/// Compact human-readable duration: `<1s` → `Nms`, `<60s` → `1.2s`,
/// otherwise `1m23s`.
pub fn format_duration(secs: f64) -> String {
    if secs < 1.0 {
        format!("{}ms", (secs * 1000.0).round() as u64)
    } else if secs < 60.0 {
        format!("{:.1}s", secs)
    } else {
        let total = secs.round() as u64;
        format!("{}m{:02}s", total / 60, total % 60)
    }
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
    fn format_duration_chooses_unit() {
        assert_eq!(format_duration(0.05), "50ms");
        assert_eq!(format_duration(4.6), "4.6s");
        assert_eq!(format_duration(124.0), "2m04s");
    }

    #[test]
    fn part_filename_strips_directory() {
        use std::path::Path;
        assert_eq!(
            part_filename(Path::new("/tmp/linux.wright.tar.zst")),
            "linux.wright.tar.zst"
        );
    }

    #[test]
    fn describe_build_capacity_pluralizes() {
        assert_eq!(
            describe_build_capacity(14, 14),
            "Forge capacity: 14 parallel tasks on 14 CPU cores."
        );
        assert_eq!(
            describe_build_capacity(1, 1),
            "Forge capacity: 1 parallel task on 1 CPU core."
        );
    }
}

//! Small, side-effect-free formatters shared across application layers.

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

/// Compact human-readable byte size (binary units, one decimal above KiB):
/// `0 B`, `512 B`, `1.5 KiB`, `40 MiB`, `2.3 GiB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_scales_and_rounds() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn build_capacity_pluralizes() {
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

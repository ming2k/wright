//! Small, side-effect-free formatters shared across application layers.

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

#[cfg(test)]
mod tests {
    use super::*;

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

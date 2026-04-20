use std::path::Path;

use crate::isolation::IsolationLevel;

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

pub fn describe_build_capacity(concurrent_tasks: usize, total_cpus: usize) -> String {
    format!(
        "Build capacity: {} parallel {} on {} {}.",
        concurrent_tasks,
        pluralize(concurrent_tasks, "task", "tasks"),
        total_cpus,
        pluralize(total_cpus, "CPU core", "CPU cores"),
    )
}

pub fn describe_batch(kind: &str, index: usize, total: usize, actions: &str) -> String {
    format!("{} batch {}/{}: {}.", kind, index, total, actions)
}

pub fn plan_scope(plan_name: &str) -> String {
    format!("[{}]", plan_name)
}

pub fn stage_started(plan_name: &str, stage_name: &str, isolation_level: IsolationLevel) -> String {
    format!(
        "{} {} started ({})",
        plan_scope(plan_name),
        stage_name,
        match isolation_level {
            IsolationLevel::None => "no isolation",
            IsolationLevel::Relaxed => "relaxed isolation",
            IsolationLevel::Strict => "strict isolation",
        }
    )
}

pub fn stage_finished(plan_name: &str, stage_name: &str, elapsed_secs: f64) -> String {
    format!(
        "{} {} done in {:.1}s",
        plan_scope(plan_name),
        stage_name,
        elapsed_secs
    )
}

pub fn build_started(plan_name: &str) -> String {
    format!("{} build started", plan_scope(plan_name))
}

pub fn build_finished(plan_name: &str) -> String {
    format!("{} build done", plan_scope(plan_name))
}

pub fn plan_packed(plan_name: &str, part_path: &Path) -> String {
    format!("{} packed {}", plan_scope(plan_name), part_path.display())
}

pub fn plan_skipped_existing(plan_name: &str) -> String {
    format!(
        "{} skipped: parts already exist (use --force to rebuild)",
        plan_scope(plan_name)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        build_finished, build_started, describe_batch, describe_build_capacity, plan_packed,
        plan_scope, plan_skipped_existing, stage_finished, stage_started,
    };
    use crate::isolation::IsolationLevel;
    use std::path::Path;

    #[test]
    fn build_log_messages_are_compact_and_scoped() {
        assert_eq!(
            describe_build_capacity(14, 14),
            "Build capacity: 14 parallel tasks on 14 CPU cores."
        );
        assert_eq!(
            describe_build_capacity(1, 1),
            "Build capacity: 1 parallel task on 1 CPU core."
        );
        assert_eq!(
            describe_batch("Build", 1, 3, "build zlib, rebuild openssl"),
            "Build batch 1/3: build zlib, rebuild openssl."
        );
        assert_eq!(plan_scope("linux"), "[linux]");
        assert_eq!(build_started("linux"), "[linux] build started");
        assert_eq!(build_finished("linux"), "[linux] build done");
        assert_eq!(
            stage_started("linux", "prepare", IsolationLevel::Strict),
            "[linux] prepare started (strict isolation)"
        );
        assert_eq!(
            stage_started("linux", "compile", IsolationLevel::None),
            "[linux] compile started (no isolation)"
        );
        assert_eq!(
            stage_finished("linux", "prepare", 4.6),
            "[linux] prepare done in 4.6s"
        );
        assert_eq!(
            plan_packed("linux", Path::new("/tmp/linux.wright.tar.zst")),
            "[linux] packed /tmp/linux.wright.tar.zst"
        );
        assert_eq!(
            plan_skipped_existing("linux"),
            "[linux] skipped: parts already exist (use --force to rebuild)"
        );
    }
}

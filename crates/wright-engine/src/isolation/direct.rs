//! Explicit host execution for `isolation = "none"`.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Stdio;

use tracing::debug;

use super::error::{IsolationError, Result};
use super::{IsolationConfig, IsolationLevel, IsolationOutput, process, resources};

pub(super) fn run(
    config: &mut IsolationConfig,
    command: &str,
    args: &[String],
) -> Result<IsolationOutput> {
    if config.level != IsolationLevel::None {
        return Err(IsolationError::InvalidConfig(
            "direct execution requires isolation level none".to_string(),
        ));
    }
    if config.base_root != Path::new("/") {
        return Err(IsolationError::UnisolatedBaseRoot(config.base_root.clone()));
    }

    debug!(
        event = "isolation.disabled",
        "Isolation disabled for this stage"
    );

    let prepared_limits = config.rlimits.prepare()?;
    let cpu_count = config.cpu_count;
    let mut child = std::process::Command::new(command);
    child
        .args(args)
        .current_dir(&config.src_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for (key, value) in &config.env {
        child.env(key, value);
    }

    // SAFETY: every value is validated before spawning. The closure performs
    // only fixed-size libc calls and constructs no formatted/heap data in the
    // post-fork child of the multi-threaded application process.
    unsafe {
        child.pre_exec(move || {
            if let Some(count) = cpu_count {
                resources::apply_cpu_affinity(count);
            }
            resources::apply_rlimits(prepared_limits)
        });
    }

    let child = child
        .spawn()
        .map_err(|error| IsolationError::io("execute direct command", error))?;
    process::supervise(child, config, true)
}

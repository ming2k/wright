mod exec;
mod mounts;
mod run;
mod scratch;

use super::IsolationConfig;

pub(super) use run::run_in_helper;

/// Compatibility entry point. New code should call
/// [`super::run_in_isolation`], which keeps the native backend private.
pub fn run_in_isolation(
    config: &mut IsolationConfig,
    command: &str,
    args: &[String],
) -> crate::error::Result<super::IsolationOutput> {
    super::run_in_isolation(config, command, args)
}

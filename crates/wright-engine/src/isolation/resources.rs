//! Resource controls shared by direct and namespace-backed execution.
//!
//! Limit values are converted and checked in the application process. The
//! application-side `pre_exec` closure therefore performs only fixed-size,
//! allocation-free libc calls after `fork`.

use std::io;

use super::ResourceLimits;
use super::error::{IsolationError, Result};

#[derive(Clone, Copy)]
pub(super) struct PreparedResourceLimits {
    address_space: Option<libc::rlim_t>,
    cpu_time: Option<libc::rlim_t>,
}

impl ResourceLimits {
    pub(super) fn prepare(&self) -> Result<PreparedResourceLimits> {
        let address_space = self
            .memory_mb
            .map(|megabytes| {
                megabytes.checked_mul(1024 * 1024).ok_or_else(|| {
                    IsolationError::InvalidConfig(format!(
                        "RLIMIT_AS value {megabytes} MiB overflows u64"
                    ))
                })
            })
            .transpose()?;

        Ok(PreparedResourceLimits {
            address_space,
            cpu_time: self.cpu_time_secs,
        })
    }
}

/// Apply limits using libc directly. All formatting and numeric validation
/// happens before this function is called.
pub(super) fn apply_rlimits(limits: PreparedResourceLimits) -> io::Result<()> {
    if let Some(bytes) = limits.address_space {
        set_limit(libc::RLIMIT_AS, bytes)?;
    }
    if let Some(seconds) = limits.cpu_time {
        set_limit(libc::RLIMIT_CPU, seconds)?;
    }
    Ok(())
}

fn set_limit(resource: libc::__rlimit_resource_t, value: libc::rlim_t) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `limit` points to an initialized `rlimit` value for the duration
    // of the call, and `resource` is one of the two constants above.
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Restrict the current process to the first `count` CPUs in its inherited
/// allowed set. Affinity remains a best-effort scheduling hint.
pub(super) fn apply_cpu_affinity(count: u32) {
    // SAFETY: both CPU sets live for the duration of the libc calls. CPU_SET
    // is only invoked with indices below CPU_SETSIZE.
    unsafe {
        let mut allowed = std::mem::zeroed::<libc::cpu_set_t>();
        if libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut allowed) != 0 {
            return;
        }

        let mut selected = std::mem::zeroed::<libc::cpu_set_t>();
        let mut selected_count = 0;
        for cpu in 0..libc::CPU_SETSIZE as usize {
            if libc::CPU_ISSET(cpu, &allowed) {
                libc::CPU_SET(cpu, &mut selected);
                selected_count += 1;
                if selected_count == count as usize {
                    break;
                }
            }
        }
        if selected_count > 0 {
            libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &selected);
        }
    }
}

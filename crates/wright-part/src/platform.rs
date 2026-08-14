//! Host platform probing for build audit data.
//!
//! [`HostInfo`] captures the machine facts that matter when diagnosing a
//! part that misbehaves after deployment (e.g. an `Illegal instruction`
//! traced back to a build host with a newer microarchitecture) or when
//! comparing build-cost records across hardware upgrades. The probe reads
//! `uname(2)`, `/proc/cpuinfo`, and `/proc/meminfo`; every field degrades
//! to a best-effort value rather than failing, because audit data must
//! never break a seal or a build.

use serde::{Deserialize, Serialize};

/// Snapshot of the build host's platform, collected at seal/build time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    /// Machine hostname (`gethostname(2)`).
    pub hostname: String,
    /// Operating system (`uname -s`, e.g. `Linux`).
    pub os: String,
    /// Kernel release (`uname -r`).
    pub kernel: String,
    /// Machine hardware name (`uname -m`, e.g. `x86_64`).
    pub arch: String,
    /// CPU model string from `/proc/cpuinfo` (`model name`, or `Model` /
    /// `Hardware` on ARM); falls back to `arch` when absent.
    pub cpu_model: String,
    /// Logical CPUs available to the process (`available_parallelism`).
    pub cpu_cores: u32,
    /// Total physical memory in bytes (`MemTotal` from `/proc/meminfo`).
    pub memory_bytes: u64,
    /// Full CPU flag set (`flags` / `Features` from `/proc/cpuinfo`).
    /// Large; recorded in `.BUILDINFO` for forensics but stripped from
    /// high-frequency ledger records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_flags: Option<String>,
}

impl HostInfo {
    /// Probe the current host. `with_flags` controls whether the (long)
    /// CPU flag list is included.
    pub fn probe(with_flags: bool) -> Self {
        let (os, kernel, arch) = uname_fields();
        let (cpu_model, cpu_flags) = cpuinfo_fields(&arch);
        HostInfo {
            hostname: hostname(),
            os,
            kernel,
            arch: arch.clone(),
            cpu_model,
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1),
            memory_bytes: memtotal_bytes().unwrap_or(0),
            cpu_flags: if with_flags { cpu_flags } else { None },
        }
    }
}

fn uname_fields() -> (String, String, String) {
    // SAFETY: a zeroed utsname is valid for libc::uname to fill; on success
    // every field is a NUL-terminated array.
    unsafe {
        let mut uts: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut uts) != 0 {
            let unknown = || "unknown".to_string();
            return (unknown(), unknown(), unknown());
        }
        let field = |raw: &[libc::c_char]| {
            let bytes: Vec<u8> = raw.iter().map(|&c| c as u8).collect();
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        };
        (
            field(&uts.sysname),
            field(&uts.release),
            field(&uts.machine),
        )
    }
}

fn hostname() -> String {
    let mut buf = vec![0u8; 256];
    // SAFETY: buf is a valid writable region of buf.len() bytes.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return "unknown".to_string();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Parse the CPU model and flag list out of `/proc/cpuinfo`. Key spelling
/// varies by architecture: x86 uses `model name` / `flags`, ARM uses
/// `Model` / `Features` or `Hardware`. Plain `model` on x86 is the numeric
/// model ID, not a name — it only serves as a fallback.
fn cpuinfo_fields(arch_fallback: &str) -> (String, Option<String>) {
    let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") else {
        return (arch_fallback.to_string(), None);
    };
    parse_cpuinfo(&content, arch_fallback)
}

fn parse_cpuinfo(content: &str, arch_fallback: &str) -> (String, Option<String>) {
    let mut model: Option<String> = None;
    let mut model_fallback: Option<String> = None;
    let mut flags: Option<String> = None;
    for line in content.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "model name" if model.is_none() => model = Some(value.to_string()),
            "model" | "hardware" if model_fallback.is_none() => {
                model_fallback = Some(value.to_string())
            }
            "flags" | "features" if flags.is_none() => flags = Some(value.to_string()),
            _ => {}
        }
    }
    (
        model
            .or(model_fallback)
            .unwrap_or_else(|| arch_fallback.to_string()),
        flags,
    )
}

fn memtotal_bytes() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_populates_core_fields() {
        let host = HostInfo::probe(true);
        assert_eq!(host.os, "Linux");
        assert!(!host.kernel.is_empty());
        assert!(!host.arch.is_empty());
        assert!(!host.cpu_model.is_empty());
        assert!(host.cpu_cores >= 1);
        assert!(host.memory_bytes > 0);
    }

    #[test]
    fn probe_without_flags_omits_them() {
        let host = HostInfo::probe(false);
        assert!(host.cpu_flags.is_none());
        // And the field disappears from JSON entirely.
        let json = serde_json::to_string(&host).unwrap();
        assert!(!json.contains("cpu_flags"));
    }

    #[test]
    fn host_info_roundtrips_through_toml() {
        let host = HostInfo::probe(true);
        let text = toml::to_string(&host).unwrap();
        let back: HostInfo = toml::from_str(&text).unwrap();
        assert_eq!(back.cpu_model, host.cpu_model);
        assert_eq!(back.memory_bytes, host.memory_bytes);
    }

    #[test]
    fn cpuinfo_prefers_model_name_over_numeric_model() {
        // x86 order: `model` (numeric ID) precedes `model name`.
        let sample = "processor   : 0\n\
                      vendor_id   : GenuineIntel\n\
                      cpu family  : 6\n\
                      model       : 197\n\
                      model name  : Intel(R) Core(TM) i7-9700\n\
                      stepping    : 13\n\
                      flags       : fpu sse sse2 avx\n";
        let (model, flags) = parse_cpuinfo(sample, "x86_64");
        assert_eq!(model, "Intel(R) Core(TM) i7-9700");
        assert_eq!(flags.as_deref(), Some("fpu sse sse2 avx"));
    }

    #[test]
    fn cpuinfo_falls_back_to_hardware_on_arm() {
        let sample = "processor   : 0\n\
                      Features    : fp asimd evtstrm\n\
                      Hardware    : BCM2835\n";
        let (model, flags) = parse_cpuinfo(sample, "aarch64");
        assert_eq!(model, "BCM2835");
        assert_eq!(flags.as_deref(), Some("fp asimd evtstrm"));

        let (model, _) = parse_cpuinfo("nothing usable\n", "riscv64");
        assert_eq!(model, "riscv64");
    }
}

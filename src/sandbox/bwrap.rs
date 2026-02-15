use std::process::{Command, Stdio};
use tracing::{debug, info};

use crate::error::{WrightError, Result};
use super::{SandboxConfig, SandboxLevel, SandboxOutput, spawn_tee_reader};

pub fn run_in_sandbox(config: &SandboxConfig, command: &str, args: &[String]) -> Result<SandboxOutput> {
    if config.level == SandboxLevel::None {
        info!("Sandbox disabled, running command directly");
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd.current_dir(&config.src_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().map_err(|e| WrightError::BuildError(format!("failed to execute command: {}", e)))?;
        let stdout_handle = spawn_tee_reader(child.stdout.take().unwrap(), std::io::stdout());
        let stderr_handle = spawn_tee_reader(child.stderr.take().unwrap(), std::io::stderr());
        let status = child.wait().map_err(|e| WrightError::BuildError(format!("failed to wait for command: {}", e)))?;
        let stdout_bytes = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();
        return Ok(SandboxOutput {
            status,
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        });
    }

    let mut bwrap = Command::new("bwrap");

    // Basic system mounts (read-only)
    for path in ["/usr", "/bin", "/sbin", "/lib", "/lib64"] {
        if std::path::Path::new(path).exists() {
            bwrap.arg("--ro-bind").arg(path).arg(path);
        }
    }

    // Essential /etc files
    for etc_file in ["/etc/ld.so.conf", "/etc/ld.so.cache", "/etc/resolv.conf", "/etc/hosts", "/etc/passwd", "/etc/group"] {
        if std::path::Path::new(etc_file).exists() {
            bwrap.arg("--ro-bind").arg(etc_file).arg(etc_file);
        }
    }

    // Mount build and output directories
    bwrap.arg("--bind").arg(&config.src_dir).arg("/build");
    bwrap.arg("--bind").arg(&config.pkg_dir).arg("/output");

    if let Some(ref patches) = config.patches_dir {
        if patches.exists() {
            bwrap.arg("--ro-bind").arg(patches).arg("/patches");
        }
    }

    // Extra binds
    for (host, dest, ro) in &config.extra_binds {
        if host.exists() {
            if *ro {
                bwrap.arg("--ro-bind").arg(host).arg(dest);
            } else {
                bwrap.arg("--bind").arg(host).arg(dest);
            }
        }
    }

    // Standard pseudo-filesystems
    bwrap.arg("--dev").arg("/dev");
    bwrap.arg("--proc").arg("/proc");
    bwrap.arg("--tmpfs").arg("/tmp");

    // Isolation levels
    match config.level {
        SandboxLevel::Strict => {
            bwrap.arg("--unshare-all");
            bwrap.arg("--share-net"); // Default to no net, but design spec says strict is no net
            // Wait, design spec 6.1 says strict has Network NS = Checked (Blocked)
            // So we use --unshare-net
            bwrap.arg("--unshare-net");
        }
        SandboxLevel::Relaxed => {
            bwrap.arg("--unshare-user");
            bwrap.arg("--unshare-pid");
            bwrap.arg("--unshare-uts");
            // Network is allowed in relaxed
        }
        SandboxLevel::None => unreachable!(),
    }

    // Safety and environment
    bwrap.arg("--die-with-parent");
    bwrap.arg("--chdir").arg("/build");

    for (key, value) in &config.env {
        bwrap.arg("--setenv").arg(key).arg(value);
    }

    // Set standard path inside sandbox
    bwrap.arg("--setenv").arg("PATH").arg("/usr/bin:/bin:/usr/sbin:/sbin");

    // Append the actual command
    bwrap.arg("--").arg(command);
    bwrap.args(args);

    bwrap.stdout(Stdio::piped());
    bwrap.stderr(Stdio::piped());

    debug!("Bwrap command: {:?}", bwrap);

    let mut child = bwrap.spawn().map_err(|e| {
        WrightError::BuildError(format!("failed to launch bubblewrap: {}", e))
    })?;

    let stdout_handle = spawn_tee_reader(child.stdout.take().unwrap(), std::io::stdout());
    let stderr_handle = spawn_tee_reader(child.stderr.take().unwrap(), std::io::stderr());

    let status = child.wait().map_err(|e| {
        WrightError::BuildError(format!("failed to wait for bubblewrap: {}", e))
    })?;

    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();

    Ok(SandboxOutput {
        status,
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
    })
}

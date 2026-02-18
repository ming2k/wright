use std::ffi::CString;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::sched::{unshare, CloneFlags};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{chdir, execvp, fork, pivot_root, sethostname, ForkResult, Pid};
use tracing::{debug, info};

use super::{ResourceLimits, SandboxConfig, SandboxLevel, SandboxOutput, spawn_tee_reader};
use crate::error::{Result, WrightError};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Spawn a watchdog thread that kills a process after `timeout` seconds.
///
/// If `kill_pgroup` is true, kills the entire process group (`kill(-pid)`).
/// Use this for unsandboxed Command-based paths where the child is a process
/// group leader (via `setpgid(0,0)` in pre_exec) — otherwise `make`/`gcc`
/// children survive the kill and become orphans.
///
/// For the fork-based sandboxed path, use `kill_pgroup = false` because the
/// PID namespace already ensures all descendants are killed when the
/// intermediate child exits.
///
/// Returns a flag that should be set to `true` when the child exits normally
/// to prevent the watchdog from firing on a recycled PID.
fn spawn_timeout_watchdog(pid: u32, timeout: u64, kill_pgroup: bool) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(timeout));
        if !done_clone.load(Ordering::Acquire) {
            let target = if kill_pgroup {
                tracing::error!(
                    "Wall-clock timeout ({timeout}s) exceeded, killing process group {pid}"
                );
                -(pid as i32)
            } else {
                tracing::error!(
                    "Wall-clock timeout ({timeout}s) exceeded, killing process {pid}"
                );
                pid as i32
            };
            unsafe { libc::kill(target, libc::SIGKILL); }
        }
    });
    done
}

/// Apply resource limits via `setrlimit`.
fn apply_rlimits(rlimits: &ResourceLimits) -> std::result::Result<(), String> {
    use nix::sys::resource::{setrlimit, Resource};

    if let Some(mb) = rlimits.memory_mb {
        let bytes = mb * 1024 * 1024;
        setrlimit(Resource::RLIMIT_AS, bytes, bytes)
            .map_err(|e| format!("setrlimit RLIMIT_AS: {e}"))?;
    }
    if let Some(secs) = rlimits.cpu_time_secs {
        setrlimit(Resource::RLIMIT_CPU, secs, secs)
            .map_err(|e| format!("setrlimit RLIMIT_CPU: {e}"))?;
    }
    Ok(())
}

/// Run a command inside a native Linux namespace sandbox.
///
/// Architecture (double-fork for PID namespace):
///
/// ```text
/// Parent
///  └─ fork() ──> Child (intermediate):
///                  unshare(NEWPID | NEWNS | NEWUSER | ...)
///                  write uid/gid maps, make mounts private
///                  fork() ──> Grandchild (PID 1 in new pidns):
///                               mount /proc (allowed as PID 1)
///                               set up newroot, bind mounts, pivot_root
///                               set env, chdir, exec(command)
///                  waitpid(grandchild) -> propagate exit status
/// ```
///
/// The double-fork is necessary because `unshare(CLONE_NEWPID)` only
/// places *children* of the calling process into the new PID namespace.
/// Mount setup and pivot_root are done in the grandchild so that /proc
/// can be mounted before pivot_root changes the filesystem root.
pub fn run_in_sandbox(
    config: &SandboxConfig,
    command: &str,
    args: &[String],
) -> Result<SandboxOutput> {
    if config.level == SandboxLevel::None {
        info!("Sandbox disabled, running command directly");
        let mut cmd = std::process::Command::new(command);
        cmd.args(args);
        cmd.current_dir(&config.src_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        let rlimits = config.rlimits.clone();
        unsafe {
            cmd.pre_exec(move || {
                // New process group so timeout can kill all descendants.
                libc::setpgid(0, 0);
                apply_rlimits(&rlimits).map_err(std::io::Error::other)
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| WrightError::SandboxError(format!("failed to execute command: {e}")))?;
        let watchdog = config.rlimits.timeout_secs.map(|t| spawn_timeout_watchdog(child.id(), t, true));
        let stdout_handle = spawn_tee_reader(child.stdout.take().unwrap(), std::io::stdout());
        let stderr_handle = spawn_tee_reader(child.stderr.take().unwrap(), std::io::stderr());
        let status = child.wait()
            .map_err(|e| WrightError::SandboxError(format!("failed to wait for command: {e}")))?;
        if let Some(done) = watchdog { done.store(true, Ordering::Release); }
        let stdout_bytes = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();
        return Ok(SandboxOutput {
            status,
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        });
    }

    let real_uid = nix::unistd::getuid();
    let real_gid = nix::unistd::getgid();
    let is_root = real_uid.is_root();

    // As root we already have all capabilities — CLONE_NEWUSER is only
    // needed for unprivileged users to gain capabilities inside the
    // namespace.  Some kernels block CLONE_NEWUSER even for root, so
    // skip it when unnecessary.
    let need_userns = !is_root;

    let mut clone_flags = match config.level {
        SandboxLevel::Strict => {
            CloneFlags::CLONE_NEWNS
                | CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWUTS
                | CloneFlags::CLONE_NEWIPC
                | CloneFlags::CLONE_NEWNET
        }
        SandboxLevel::Relaxed => {
            CloneFlags::CLONE_NEWNS
                | CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWUTS
        }
        SandboxLevel::None => unreachable!(),
    };

    if need_userns {
        clone_flags |= CloneFlags::CLONE_NEWUSER;
    }

    // Probe whether the required namespaces are available.
    if !can_unshare(clone_flags) {
        tracing::warn!(
            "Namespace isolation unavailable (unshare blocked by kernel/container); \
             falling back to direct execution"
        );
        let mut cmd = std::process::Command::new(command);
        cmd.args(args);
        cmd.current_dir(&config.src_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        let rlimits = config.rlimits.clone();
        unsafe {
            cmd.pre_exec(move || {
                libc::setpgid(0, 0);
                apply_rlimits(&rlimits).map_err(std::io::Error::other)
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| WrightError::SandboxError(format!("failed to execute command: {e}")))?;
        let watchdog = config.rlimits.timeout_secs.map(|t| spawn_timeout_watchdog(child.id(), t, true));
        let stdout_handle = spawn_tee_reader(child.stdout.take().unwrap(), std::io::stdout());
        let stderr_handle = spawn_tee_reader(child.stderr.take().unwrap(), std::io::stderr());
        let status = child.wait()
            .map_err(|e| WrightError::SandboxError(format!("failed to wait for command: {e}")))?;
        if let Some(done) = watchdog { done.store(true, Ordering::Release); }
        let stdout_bytes = stdout_handle.join().unwrap_or_default();
        let stderr_bytes = stderr_handle.join().unwrap_or_default();
        return Ok(SandboxOutput {
            status,
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        });
    }

    // Error pipe: child/grandchild write error messages, parent reads.
    let (err_read, err_write) =
        nix::unistd::pipe().map_err(|e| WrightError::SandboxError(format!("pipe: {e}")))?;
    let err_write_fd = err_write.as_raw_fd();

    // Stdout/stderr pipes: grandchild writes, parent reads + tees.
    let (out_read, out_write) =
        nix::unistd::pipe().map_err(|e| WrightError::SandboxError(format!("pipe: {e}")))?;
    let out_write_fd = out_write.as_raw_fd();
    let (eout_read, eout_write) =
        nix::unistd::pipe().map_err(|e| WrightError::SandboxError(format!("pipe: {e}")))?;
    let eout_write_fd = eout_write.as_raw_fd();

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            drop(err_read);
            drop(out_read);
            drop(eout_read);

            let die = |msg: String| -> ! {
                let bytes = msg.as_bytes();
                let _ = nix::unistd::write(
                    unsafe { std::os::fd::BorrowedFd::borrow_raw(err_write_fd) },
                    bytes,
                );
                drop(unsafe { OwnedFd::from_raw_fd(err_write_fd) });
                unsafe { libc::_exit(1) }
            };

            unsafe {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            }

            // --- Unshare namespaces ---
            if let Err(e) = unshare(clone_flags) {
                die(format!("unshare: {e}"));
            }

            // --- Write uid/gid maps ---
            if clone_flags.contains(CloneFlags::CLONE_NEWUSER) {
                if let Err(e) = std::fs::write("/proc/self/setgroups", "deny") {
                    die(format!("write setgroups: {e}"));
                }
                if let Err(e) =
                    std::fs::write("/proc/self/uid_map", format!("0 {real_uid} 1\n"))
                {
                    die(format!("write uid_map: {e}"));
                }
                if let Err(e) =
                    std::fs::write("/proc/self/gid_map", format!("0 {real_gid} 1\n"))
                {
                    die(format!("write gid_map: {e}"));
                }
            }

            // --- Make mounts private ---
            if let Err(e) = mount(
                None::<&str>,
                "/",
                None::<&str>,
                MsFlags::MS_REC | MsFlags::MS_PRIVATE,
                None::<&str>,
            ) {
                die(format!("mount MS_PRIVATE /: {e}"));
            }

            // --- Double-fork: grandchild is PID 1 in new PID namespace ---
            // All mount setup + pivot_root happens in the grandchild so
            // that /proc can be mounted while we're still PID 1 with access
            // to the host filesystem (before pivot_root).

            match unsafe { fork() } {
                Ok(ForkResult::Child) => {
                    // Grandchild — PID 1 in the new PID namespace.
                    unsafe {
                        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    }

                    // Mount a fresh /proc for our PID namespace (before
                    // pivot_root — same approach as `unshare --mount-proc`).
                    if let Err(e) = mount(
                        Some("proc"),
                        "/proc",
                        Some("proc"),
                        MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
                        None::<&str>,
                    ) {
                        die(format!("mount proc: {e}"));
                    }

                    // --- Set up new root filesystem ---

                    let newroot = PathBuf::from(format!("/tmp/.wright-sandbox-root-{}", config.task_id));
                    if let Err(e) = std::fs::create_dir_all(&newroot) {
                        die(format!("mkdir newroot: {e}"));
                    }

                    // Try OverlayFS first (much faster and cleaner)
                    let mut overlay_success = false;
                    let overlay_base = PathBuf::from(format!("/tmp/wright-overlay-{}", config.task_id));
                    let upper = overlay_base.join("upper");
                    let work = overlay_base.join("work");

                    if std::fs::create_dir_all(&upper).is_ok() && std::fs::create_dir_all(&work).is_ok() {
                        let opts = format!(
                            "lowerdir=/,upperdir={},workdir={}",
                            upper.to_string_lossy(),
                            work.to_string_lossy()
                        );
                        if mount(
                            Some("overlay"),
                            &newroot,
                            Some("overlay"),
                            MsFlags::empty(),
                            Some(opts.as_str()),
                        ).is_ok() {
                            overlay_success = true;
                            debug!("Using OverlayFS for sandbox root: {}", newroot.display());
                        }
                    }

                    if !overlay_success {
                        debug!("OverlayFS failed, falling back to tmpfs + bind mounts");
                        if let Err(e) = mount(
                            Some("tmpfs"),
                            &newroot,
                            Some("tmpfs"),
                            MsFlags::empty(),
                            None::<&str>,
                        ) {
                            die(format!("mount tmpfs on newroot: {e}"));
                        }
                    }

                    // Helper to bind-mount a path into the new root.
                    let bind = |src: &Path,
                                dest_rel: &str,
                                readonly: bool|
                     -> std::result::Result<(), String> {
                        let dest = newroot.join(dest_rel.trim_start_matches('/'));
                        
                        // Fix: ALWAYS ensure the destination mount point exists.
                        // Even with overlay, we need to create the directory/file in the upperdir.
                        if src.is_dir() {
                            std::fs::create_dir_all(&dest)
                                .map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
                        } else {
                            if let Some(parent) = dest.parent() {
                                std::fs::create_dir_all(parent)
                                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
                            }
                            std::fs::write(&dest, b"")
                                .map_err(|e| format!("touch {}: {e}", dest.display()))?;
                        }

                        mount(
                            Some(src),
                            &dest,
                            None::<&str>,
                            MsFlags::MS_BIND | MsFlags::MS_REC,
                            None::<&str>,
                        )
                        .map_err(|e| {
                            format!(
                                "bind mount {} -> {}: {e}",
                                src.display(),
                                dest.display()
                            )
                        })?;

                        if readonly {
                            mount(
                                None::<&str>,
                                &dest,
                                None::<&str>,
                                MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
                                None::<&str>,
                            )
                            .map_err(|e| format!("remount ro {}: {e}", dest.display()))?;
                        }
                        Ok(())
                    };

                    // If not using overlay, we must manually bind system dirs.
                    // If using overlay, these are already present via lowerdir=/.
                    if !overlay_success {
                        for dir in ["/usr", "/bin", "/sbin", "/lib", "/lib64"] {
                            let p = Path::new(dir);
                            let dest = newroot.join(dir.trim_start_matches('/'));
                            if let Ok(target) = std::fs::read_link(p) {
                                if let Err(e) = std::os::unix::fs::symlink(&target, &dest) {
                                    if e.kind() != std::io::ErrorKind::AlreadyExists {
                                        die(format!("symlink {} -> {}: {e}", dest.display(), target.display()));
                                    }
                                }
                            } else if p.exists() {
                                if let Err(e) = bind(p, dir, true) {
                                    die(e);
                                }
                            }
                        }

                        // Essential /etc files.
                        for etc_file in ["/etc/ld.so.conf", "/etc/ld.so.cache", "/etc/resolv.conf", "/etc/hosts", "/etc/passwd", "/etc/group"] {
                            let p = Path::new(etc_file);
                            if p.exists() {
                                if let Err(e) = bind(p, etc_file, true) {
                                    die(e);
                                }
                            }
                        }
                    }

                    // Build and output directories (read-write).
                    if let Err(e) = bind(&config.src_dir, "/build", false) {
                        die(e);
                    }
                    if let Err(e) = bind(&config.pkg_dir, "/output", false) {
                        die(e);
                    }

                    // Files directory (read-only, optional).
                    if let Some(ref files) = config.files_dir {
                        if files.exists() {
                            if let Err(e) = bind(files, "/files", true) {
                                die(e);
                            }
                        }
                    }

                    // Extra binds.
                    for (host, dest, ro) in &config.extra_binds {
                        if host.exists() {
                            if let Err(e) = bind(host, &dest.to_string_lossy(), *ro) {
                                die(e);
                            }
                        }
                    }

                    // /dev: try devtmpfs, fall back to tmpfs + bind-mounted devices.
                    let dev = newroot.join("dev");
                    std::fs::create_dir_all(&dev).ok();
                    if mount(
                        Some("devtmpfs"),
                        &dev,
                        Some("devtmpfs"),
                        MsFlags::empty(),
                        None::<&str>,
                    )
                    .is_err()
                    {
                        let _ = mount(
                            Some("tmpfs"),
                            &dev,
                            Some("tmpfs"),
                            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
                            Some("mode=0755"),
                        );
                        for devname in ["null", "zero", "urandom", "random", "full"] {
                            let host_dev = PathBuf::from(format!("/dev/{devname}"));
                            let new_dev = dev.join(devname);
                            if host_dev.exists() {
                                std::fs::write(&new_dev, b"").ok();
                                let _ = mount(
                                    Some(host_dev.as_path()),
                                    &new_dev,
                                    None::<&str>,
                                    MsFlags::MS_BIND,
                                    None::<&str>,
                                );
                            }
                        }
                    }

                    // /proc: bind-mount the fresh proc we mounted earlier.
                    let proc_dir = newroot.join("proc");
                    std::fs::create_dir_all(&proc_dir).ok();
                    if let Err(e) = mount(
                        Some("/proc"),
                        &proc_dir,
                        None::<&str>,
                        MsFlags::MS_BIND | MsFlags::MS_REC,
                        None::<&str>,
                    ) {
                        die(format!("bind mount /proc: {e}"));
                    }

                    // /tmp
                    let tmp = newroot.join("tmp");
                    std::fs::create_dir_all(&tmp).ok();
                    let _ = mount(
                        Some("tmpfs"),
                        &tmp,
                        Some("tmpfs"),
                        MsFlags::empty(),
                        None::<&str>,
                    );

                    // --- pivot_root ---

                    let old_root = newroot.join(".old_root");
                    std::fs::create_dir_all(&old_root).ok();

                    if let Err(e) = pivot_root(&newroot, &old_root) {
                        die(format!("pivot_root: {e}"));
                    }

                    if let Err(e) = chdir("/") {
                        die(format!("chdir /: {e}"));
                    }
                    let _ = umount2("/.old_root", MntFlags::MNT_DETACH);
                    let _ = std::fs::remove_dir("/.old_root");

                    // --- Hostname ---
                    let _ = sethostname("wright-sandbox");

                    // --- Environment ---
                    for (key, _) in std::env::vars_os() {
                        std::env::remove_var(&key);
                    }
                    std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
                    std::env::set_var("HOME", "/build");
                    std::env::set_var("TERM", "xterm");
                    for (key, value) in &config.env {
                        std::env::set_var(key, value);
                    }

                    // --- chdir + exec ---
                    if let Err(e) = chdir("/build") {
                        die(format!("chdir /build: {e}"));
                    }

                    let c_command = CString::new(command)
                        .map_err(|e| format!("invalid command: {e}"))
                        .unwrap_or_else(|e| {
                            die(e);
                        });

                    let mut c_args: Vec<CString> = Vec::with_capacity(args.len() + 1);
                    c_args.push(c_command.clone());
                    for arg in args {
                        match CString::new(arg.as_str()) {
                            Ok(c) => c_args.push(c),
                            Err(e) => die(format!("invalid argument: {e}")),
                        }
                    }

                    // Redirect stdout/stderr to pipes for capture.
                    unsafe {
                        libc::dup2(out_write_fd, 1);
                        libc::dup2(eout_write_fd, 2);
                    }
                    // Close all pipe fds (originals no longer needed after dup2).
                    std::mem::forget(out_write);
                    let _ = nix::unistd::close(out_write_fd);
                    std::mem::forget(eout_write);
                    let _ = nix::unistd::close(eout_write_fd);

                    // Close error pipe before exec.
                    std::mem::forget(err_write);
                    let _ = nix::unistd::close(err_write_fd);

                    // Apply resource limits before exec.
                    if let Err(e) = apply_rlimits(&config.rlimits) {
                        eprintln!("rlimits: {e}");
                        unsafe { libc::_exit(1) }
                    }

                    // Retry loop for ETXTBSY (Text file busy).
                    // This happens if an interpreter (like /bin/sh) is being overwritten 
                    // by the host while we try to exec it in the sandbox.
                    let mut retries = 0;
                    loop {
                        match execvp(&c_command, &c_args) {
                            Ok(infallible) => match infallible {},
                            Err(e) if e == nix::errno::Errno::ETXTBSY && retries < 10 => {
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                retries += 1;
                                continue;
                            }
                            Err(e) => {
                                eprintln!("exec {command}: {e}");
                                unsafe { libc::_exit(127) }
                            }
                        }
                    }
                }
                Ok(ForkResult::Parent { child: grandchild }) => {
                    // Intermediate child: wait for grandchild, propagate exit.
                    // Close all pipe fds — we don't use them here.
                    std::mem::forget(out_write);
                    let _ = nix::unistd::close(out_write_fd);
                    std::mem::forget(eout_write);
                    let _ = nix::unistd::close(eout_write_fd);
                    std::mem::forget(err_write);
                    let _ = nix::unistd::close(err_write_fd);

                    match wait_for_raw_status(grandchild) {
                        Ok(raw) => unsafe { libc::_exit(raw) },
                        Err(_) => unsafe { libc::_exit(1) },
                    }
                }
                Err(e) => {
                    die(format!("inner fork: {e}"));
                }
            }
        }
        Ok(ForkResult::Parent { child }) => {
            drop(err_write);
            drop(out_write);
            drop(eout_write);

            let mut err_buf = vec![0u8; 4096];
            let n = nix::unistd::read(err_read.as_raw_fd(), &mut err_buf).unwrap_or(0);
            drop(err_read);

            if n > 0 {
                let msg = String::from_utf8_lossy(&err_buf[..n]).to_string();
                let _ = waitpid(child, None);
                return Err(WrightError::SandboxError(format!(
                    "sandbox setup failed: {msg}"
                )));
            }

            // Spawn tee readers to capture + echo stdout/stderr in real time.
            let out_file = unsafe { std::fs::File::from_raw_fd(out_read.as_raw_fd()) };
            std::mem::forget(out_read); // Ownership transferred to File
            let err_file = unsafe { std::fs::File::from_raw_fd(eout_read.as_raw_fd()) };
            std::mem::forget(eout_read);

            let watchdog = config.rlimits.timeout_secs.map(|t| {
                spawn_timeout_watchdog(child.as_raw() as u32, t, false)
            });

            let stdout_handle = spawn_tee_reader(out_file, std::io::stdout());
            let stderr_handle = spawn_tee_reader(err_file, std::io::stderr());

            let status = wait_for_child(child)?;
            if let Some(done) = watchdog { done.store(true, Ordering::Release); }

            let stdout_bytes = stdout_handle.join().unwrap_or_default();
            let stderr_bytes = stderr_handle.join().unwrap_or_default();

            debug!("Sandbox child exited with: {:?}", status);
            Ok(SandboxOutput {
                status,
                stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
                stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
            })
        }
        Err(e) => Err(WrightError::SandboxError(format!("fork: {e}"))),
    }
}

/// Wait for a child and return the raw exit code (0-255).
fn wait_for_raw_status(pid: Pid) -> std::result::Result<i32, ()> {
    loop {
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_pid, code)) => return Ok(code),
            Ok(WaitStatus::Signaled(_pid, sig, _core)) => return Ok(128 + sig as i32),
            Ok(WaitStatus::Stopped(..)) | Ok(WaitStatus::Continued(..)) => continue,
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => return Err(()),
        }
    }
}

/// Wait for a child process and convert the result to `ExitStatus`.
fn wait_for_child(pid: Pid) -> Result<ExitStatus> {
    loop {
        match waitpid(pid, None) {
            Ok(WaitStatus::Exited(_pid, code)) => {
                use std::os::unix::process::ExitStatusExt;
                return Ok(ExitStatus::from_raw(code << 8));
            }
            Ok(WaitStatus::Signaled(_pid, sig, _core)) => {
                use std::os::unix::process::ExitStatusExt;
                return Ok(ExitStatus::from_raw(sig as i32));
            }
            Ok(WaitStatus::Stopped(..)) | Ok(WaitStatus::Continued(..)) => {
                continue;
            }
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => {
                return Err(WrightError::SandboxError(format!("waitpid: {e}")));
            }
        }
    }
}

/// Quick probe: can we create the required namespaces?
///
/// Fork a throwaway child that attempts `unshare(flags)`.
/// Returns true if the child succeeds, false otherwise.
/// This detects environments that block namespace creation.
fn can_unshare(flags: CloneFlags) -> bool {
    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            let ok = unshare(flags).is_ok();
            unsafe { libc::_exit(if ok { 0 } else { 1 }) }
        }
        Ok(ForkResult::Parent { child }) => {
            matches!(
                waitpid(child, None),
                Ok(WaitStatus::Exited(_, 0))
            )
        }
        Err(_) => false,
    }
}

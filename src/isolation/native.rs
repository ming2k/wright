use std::ffi::CString;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use nix::mount::{MntFlags, MsFlags, mount, umount2};
use nix::sched::{CloneFlags, unshare};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, chdir, execvp, fork, pivot_root, sethostname};
use tracing::debug;

use super::{
    CapturedOutput, IsolationConfig, IsolationLevel, IsolationOutput, ResourceLimits,
    spawn_stream_reader,
};
use crate::error::{Result, WrightError};

/// Create a stream reader that captures output to a temp file, optionally
/// echoing to the terminal and/or teeing to a log file in real time.
/// Returns a [`CapturedOutput`] with the tail in memory and the full content
/// on disk.
fn make_stream_capture<R: std::io::Read + Send + 'static>(
    source: R,
    verbose: bool,
    log_sink: Option<std::fs::File>,
) -> std::thread::JoinHandle<CapturedOutput> {
    let dest = tempfile::tempfile().expect("failed to create capture temp file");
    let echo: Option<Box<dyn std::io::Write + Send>> = if verbose {
        Some(Box::new(std::io::stderr()))
    } else {
        None
    };
    let log_to: Option<Box<dyn std::io::Write + Send>> =
        log_sink.map(|f| Box::new(f) as Box<dyn std::io::Write + Send>);
    spawn_stream_reader(source, echo, log_to, dest)
}

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Spawn a watchdog thread that kills a process after `timeout` seconds.
///
/// If `kill_pgroup` is true, kills the entire process group (`kill(-pid)`).
/// Use this for undocked Command-based paths where the child is a process
/// group leader (via `setpgid(0,0)` in pre_exec) — otherwise `make`/`gcc`
/// children survive the kill and become orphans.
///
/// For the fork-based isolation path, use `kill_pgroup = false` because the
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
                tracing::error!("Wall-clock timeout ({timeout}s) exceeded, killing process {pid}");
                pid as i32
            };
            unsafe {
                libc::kill(target, libc::SIGKILL);
            }
        }
    });
    done
}

/// Pin the calling process to the first `n` CPUs via `sched_setaffinity`.
///
/// Called in `pre_exec` (for direct-execution paths) and inside the isolation
/// grandchild (for the sandboxed path) so that `nproc` — which reads
/// `sched_getaffinity` on Linux — returns the scheduler's computed share
/// rather than the full host count.  Errors are silently ignored: affinity is
/// a best-effort resource hint, not a hard correctness requirement.
fn apply_cpu_affinity(n: u32) {
    unsafe {
        let total = libc::sysconf(libc::_SC_NPROCESSORS_ONLN).max(1) as u32;
        let count = n.min(total).max(1);
        let mut set = std::mem::zeroed::<libc::cpu_set_t>();
        for i in 0..count {
            libc::CPU_SET(i as usize, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

/// Apply resource limits via `setrlimit`.
fn apply_rlimits(rlimits: &ResourceLimits) -> std::result::Result<(), String> {
    use nix::sys::resource::{Resource, setrlimit};

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

/// Derive the scratch directory for isolation setup from the active build root.
///
/// `src_dir` is `<build_root>/src` for normal builds, so placing scratch
/// directories under its parent keeps temporary overlay state on the same
/// filesystem as the rest of the build instead of hardcoding `/tmp`.
fn isolation_scratch_base(config: &IsolationConfig) -> PathBuf {
    let build_root = config.src_dir.parent().unwrap_or(config.src_dir.as_path());
    build_root.join(".wright-isolation").join(&config.task_id)
}

/// Remove the temporary overlay and isolation-root directories for a given task.
///
/// These directories are created inside the forked child's mount namespace.
/// The mounts are automatically cleaned up when the namespace is destroyed,
/// but the empty directory trees can persist on the host filesystem after
/// crashes or forced termination.
fn cleanup_isolation_dirs(config: &IsolationConfig) {
    let scratch = isolation_scratch_base(config);
    if scratch.exists() {
        if let Err(e) = std::fs::remove_dir_all(&scratch) {
            debug!("Failed to clean up {}: {}", scratch.display(), e);
        }
    }
}

/// Run a command inside a native Linux namespace isolation.
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
pub fn run_in_isolation(
    config: &mut IsolationConfig,
    command: &str,
    args: &[String],
) -> Result<IsolationOutput> {
    if config.level == IsolationLevel::None {
        if config.base_root != Path::new("/") {
            return Err(WrightError::IsolationError(format!(
                "isolation level none cannot run against base root {}",
                config.base_root.display()
            )));
        }
        debug!("Isolation isolation disabled for this stage");
        let mut cmd = std::process::Command::new(command);
        cmd.args(args);
        cmd.current_dir(&config.src_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        for (key, value) in &config.env {
            cmd.env(key, value);
        }
        let rlimits = config.rlimits.clone();
        let cpu_count = config.cpu_count;
        unsafe {
            cmd.pre_exec(move || {
                // New process group so timeout can kill all descendants.
                libc::setpgid(0, 0);
                // Pin to the scheduler's CPU share so `nproc` returns the
                // correct count even without namespace isolation.
                if let Some(n) = cpu_count {
                    apply_cpu_affinity(n);
                }
                apply_rlimits(&rlimits).map_err(std::io::Error::other)
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| WrightError::IsolationError(format!("failed to execute command: {e}")))?;
        let watchdog = config
            .rlimits
            .timeout_secs
            .map(|t| spawn_timeout_watchdog(child.id(), t, true));
        let stdout_handle = make_stream_capture(
            child.stdout.take().unwrap(),
            config.verbose,
            config.log_stdout.take(),
        );
        let stderr_handle = make_stream_capture(
            child.stderr.take().unwrap(),
            config.verbose,
            config.log_stderr.take(),
        );
        let status = child
            .wait()
            .map_err(|e| WrightError::IsolationError(format!("failed to wait for command: {e}")))?;
        if let Some(done) = watchdog {
            done.store(true, Ordering::Release);
        }
        let empty = || CapturedOutput {
            file: tempfile::tempfile().unwrap(),
            tail: String::new(),
        };
        let stdout = stdout_handle.join().unwrap_or_else(|_| empty());
        let stderr = stderr_handle.join().unwrap_or_else(|_| empty());
        return Ok(IsolationOutput {
            status,
            stdout,
            stderr,
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
        IsolationLevel::Strict => {
            CloneFlags::CLONE_NEWNS
                | CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWUTS
                | CloneFlags::CLONE_NEWIPC
                | CloneFlags::CLONE_NEWNET
        }
        IsolationLevel::Relaxed => {
            CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS
        }
        IsolationLevel::None => unreachable!(),
    };

    if need_userns {
        clone_flags |= CloneFlags::CLONE_NEWUSER;
    }

    // Probe whether the required namespaces are available.
    if !can_unshare(clone_flags) {
        if config.base_root != Path::new("/") {
            return Err(WrightError::IsolationError(format!(
                "namespace isolation unavailable, cannot run against base root {}",
                config.base_root.display()
            )));
        }
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
        let cpu_count = config.cpu_count;
        unsafe {
            cmd.pre_exec(move || {
                libc::setpgid(0, 0);
                if let Some(n) = cpu_count {
                    apply_cpu_affinity(n);
                }
                apply_rlimits(&rlimits).map_err(std::io::Error::other)
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| WrightError::IsolationError(format!("failed to execute command: {e}")))?;
        let watchdog = config
            .rlimits
            .timeout_secs
            .map(|t| spawn_timeout_watchdog(child.id(), t, true));
        let stdout_handle = make_stream_capture(
            child.stdout.take().unwrap(),
            config.verbose,
            config.log_stdout.take(),
        );
        let stderr_handle = make_stream_capture(
            child.stderr.take().unwrap(),
            config.verbose,
            config.log_stderr.take(),
        );
        let status = child
            .wait()
            .map_err(|e| WrightError::IsolationError(format!("failed to wait for command: {e}")))?;
        if let Some(done) = watchdog {
            done.store(true, Ordering::Release);
        }
        let empty = || CapturedOutput {
            file: tempfile::tempfile().unwrap(),
            tail: String::new(),
        };
        let stdout = stdout_handle.join().unwrap_or_else(|_| empty());
        let stderr = stderr_handle.join().unwrap_or_else(|_| empty());
        return Ok(IsolationOutput {
            status,
            stdout,
            stderr,
        });
    }

    // Error pipe: child/grandchild write error messages, parent reads.
    let (err_read, err_write) =
        nix::unistd::pipe().map_err(|e| WrightError::IsolationError(format!("pipe: {e}")))?;
    let err_write_fd = err_write.as_raw_fd();

    // Stdout/stderr pipes: grandchild writes, parent reads + tees.
    let (out_read, out_write) =
        nix::unistd::pipe().map_err(|e| WrightError::IsolationError(format!("pipe: {e}")))?;
    let out_write_fd = out_write.as_raw_fd();
    let (eout_read, eout_write) =
        nix::unistd::pipe().map_err(|e| WrightError::IsolationError(format!("pipe: {e}")))?;
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
                if let Err(e) = std::fs::write("/proc/self/uid_map", format!("0 {real_uid} 1\n")) {
                    die(format!("write uid_map: {e}"));
                }
                if let Err(e) = std::fs::write("/proc/self/gid_map", format!("0 {real_gid} 1\n")) {
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
                    //
                    // OverlayFS with multiple read-only lowerdirs (host system
                    // directories) and a per-task writable upperdir.  Build
                    // output goes to /build and /output (per-task bind mounts),
                    // so any writes to system paths are captured in the per-task
                    // upper layer via copy-up.

                    let scratch_base = isolation_scratch_base(config);
                    let newroot = scratch_base.join("root");
                    if let Err(e) = std::fs::create_dir_all(&newroot) {
                        die(format!("mkdir newroot: {e}"));
                    }

                    let upper = scratch_base.join("upper");
                    let work = scratch_base.join("work");

                    if let Err(e) = std::fs::create_dir_all(&upper) {
                        die(format!("mkdir overlay upper {}: {e}", upper.display()));
                    }
                    if let Err(e) = std::fs::create_dir_all(&work) {
                        die(format!("mkdir overlay work {}: {e}", work.display()));
                    }

                    let lowerdir = if config.base_root == Path::new("/") {
                        let system_dirs = ["/usr", "/bin", "/sbin", "/lib", "/lib64"];
                        let mut seen = std::collections::HashSet::new();
                        let mut parts: Vec<PathBuf> = Vec::new();
                        for d in system_dirs {
                            let p = Path::new(d);
                            if !p.exists() {
                                continue;
                            }
                            let resolved =
                                std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
                            if seen.insert(resolved.clone()) {
                                parts.push(resolved);
                            }
                        }
                        // Drop subdirectories: on merged-/usr systems /bin→/usr/bin
                        // sits under /usr, so /usr alone suffices.
                        let mut keep: Vec<&PathBuf> = Vec::new();
                        for r in &parts {
                            if !keep.iter().any(|q| r.starts_with(q)) {
                                keep.push(r);
                            }
                        }
                        keep.iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(":")
                    } else {
                        config.base_root.display().to_string()
                    };

                    let opts = format!(
                        "lowerdir={},upperdir={},workdir={}",
                        lowerdir,
                        upper.display(),
                        work.display(),
                    );

                    debug!(
                        "Mounting overlayfs: lowerdir={} upperdir={} workdir={}",
                        lowerdir,
                        upper.display(),
                        work.display(),
                    );

                    if let Err(e) = mount(
                        Some("overlay"),
                        &newroot,
                        Some("overlay"),
                        MsFlags::empty(),
                        Some(opts.as_str()),
                    ) {
                        die(format!("overlayfs mount on {}: {e}", newroot.display(),));
                    }

                    // Helper to bind-mount a path into the new root.
                    let bind = |src: &Path,
                                dest_rel: &str,
                                readonly: bool|
                     -> std::result::Result<(), String> {
                        let dest = newroot.join(dest_rel.trim_start_matches('/'));

                        // If it's a symlink, remove it so we can mount over it properly
                        // instead of mounting onto a potentially dangling target (e.g.
                        // /etc/resolv.conf -> /run/... when /run is a fresh tmpfs).
                        if let Ok(meta) = dest.symlink_metadata() {
                            if meta.file_type().is_symlink() {
                                let _ = std::fs::remove_file(&dest);
                            }
                        }

                        // Fix: ALWAYS ensure the destination mount point exists.
                        if src.is_dir() {
                            if dest.symlink_metadata().is_err() {
                                std::fs::create_dir_all(&dest)
                                    .map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
                            }
                        } else if dest.symlink_metadata().is_err() {
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
                            format!("bind mount {} -> {}: {e}", src.display(), dest.display())
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

                    // Build and output directories (read-write).
                    if let Err(e) = bind(&config.src_dir, "/build", false) {
                        die(e);
                    }
                    if let Err(e) = bind(&config.output_dir, "/output", false) {
                        die(e);
                    }

                    // Extra binds.
                    for (host, dest, ro) in &config.extra_binds {
                        if host.exists() {
                            if let Err(e) = bind(host, &dest.to_string_lossy(), *ro) {
                                die(e);
                            }
                        }
                    }
                    // Build dependency mounts (read-only).
                    for (host, dest) in &config.dep_mounts {
                        if host.exists() {
                            if let Err(e) = bind(host, &dest.to_string_lossy(), true) {
                                die(e);
                            }
                        }
                    }
                    // On merged-/usr systems the lowerdir collapses to a single
                    // directory (e.g. /usr), which overlayfs flattens so that
                    // /usr/lib appears as /lib and there is no /usr directory.
                    // The host's ld.so.cache (bind-mounted below) contains
                    // absolute paths like /usr/lib/..., which would fail to
                    // resolve.  Bind-mount the host /usr to restore the
                    // expected hierarchy.
                    if newroot.join("usr").metadata().is_err() {
                        if let Err(e) = bind(Path::new("/usr"), "/usr", true) {
                            die(e);
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

                    // /run
                    let run_dir = newroot.join("run");
                    if let Err(e) = std::fs::create_dir_all(&run_dir) {
                        die(format!("mkdir {}: {e}", run_dir.display()));
                    }
                    if let Err(e) = mount(
                        Some("tmpfs"),
                        &run_dir,
                        Some("tmpfs"),
                        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
                        Some("mode=0755"),
                    ) {
                        die(format!("mount tmpfs on /run: {e}"));
                    }

                    // /tmp
                    let tmp = newroot.join("tmp");
                    if let Err(e) = std::fs::create_dir_all(&tmp) {
                        die(format!("mkdir {}: {e}", tmp.display()));
                    }
                    if let Err(e) = mount(
                        Some("tmpfs"),
                        &tmp,
                        Some("tmpfs"),
                        MsFlags::empty(),
                        None::<&str>,
                    ) {
                        die(format!("mount tmpfs on /tmp: {e}"));
                    }

                    // --- Essential /etc files ---
                    // Always bind-mount these to ensure they are available and correct,
                    // especially when /etc/resolv.conf is a symlink to /run which we masked.
                    for etc_file in [
                        "/etc/ld.so.conf",
                        "/etc/ld.so.cache",
                        "/etc/resolv.conf",
                        "/etc/hosts",
                        "/etc/passwd",
                        "/etc/group",
                        "/etc/ssl",
                    ] {
                        let p = Path::new(etc_file);
                        if p.exists() {
                            if let Err(e) = bind(p, etc_file, true) {
                                die(e);
                            }
                        }
                    }

                    // --- pivot_root ---

                    let old_root = newroot.join(".old_root");
                    if old_root.symlink_metadata().is_err() {
                        if let Err(e) = std::fs::create_dir_all(&old_root) {
                            die(format!("mkdir {}: {e}", old_root.display()));
                        }
                    }

                    if let Err(e) = pivot_root(&newroot, &old_root) {
                        die(format!("pivot_root: {e}"));
                    }
                    if let Err(e) = chdir("/") {
                        die(format!("chdir /: {e}"));
                    }
                    let _ = umount2("/.old_root", MntFlags::MNT_DETACH);
                    let _ = std::fs::remove_dir("/.old_root");

                    // --- Hostname ---
                    let _ = sethostname("wright-isolation");

                    // --- Environment ---
                    for (key, _) in std::env::vars_os() {
                        unsafe { std::env::remove_var(&key) };
                    }
                    unsafe { std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin") };
                    unsafe { std::env::set_var("HOME", "/build") };
                    unsafe { std::env::set_var("TERM", "xterm") };
                    for (key, value) in &config.env {
                        unsafe { std::env::set_var(key, value) };
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

                    // Pin this process to N CPUs so that `nproc` inside the
                    // isolation returns the scheduler's computed share rather than
                    // the full host count.
                    if let Some(n) = config.cpu_count {
                        apply_cpu_affinity(n);
                    }

                    // Defensive retry for ETXTBUSY: multiple lowerdirs may
                    // kernels or filesystem configurations may briefly report the
                    // file as busy.  A short exponential backoff covers the window.
                    for attempt in 0..8 {
                        match execvp(&c_command, &c_args) {
                            Ok(infallible) => match infallible {},
                            Err(nix::errno::Errno::ETXTBSY) if attempt < 7 => {
                                let delay_ms = 50 * (1_u64 << attempt);
                                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                            }
                            Err(e) => {
                                eprintln!("exec {command}: {e}");
                                unsafe { libc::_exit(127) }
                            }
                        }
                    }
                    // All retries exhausted.
                    eprintln!("exec {command}: ETXTBUSY after retries");
                    unsafe { libc::_exit(127) }
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
                cleanup_isolation_dirs(config);
                return Err(WrightError::IsolationError(format!(
                    "isolation setup failed: {msg}"
                )));
            }

            // Spawn tee readers to capture + echo stdout/stderr in real time.
            let out_file = unsafe { std::fs::File::from_raw_fd(out_read.as_raw_fd()) };
            std::mem::forget(out_read); // Ownership transferred to File
            let err_file = unsafe { std::fs::File::from_raw_fd(eout_read.as_raw_fd()) };
            std::mem::forget(eout_read);

            let watchdog = config
                .rlimits
                .timeout_secs
                .map(|t| spawn_timeout_watchdog(child.as_raw() as u32, t, false));

            let stdout_handle =
                make_stream_capture(out_file, config.verbose, config.log_stdout.take());
            let stderr_handle =
                make_stream_capture(err_file, config.verbose, config.log_stderr.take());

            let status = wait_for_child(child)?;
            if let Some(done) = watchdog {
                done.store(true, Ordering::Release);
            }

            let empty = || CapturedOutput {
                file: tempfile::tempfile().unwrap(),
                tail: String::new(),
            };
            let stdout = stdout_handle.join().unwrap_or_else(|_| empty());
            let stderr = stderr_handle.join().unwrap_or_else(|_| empty());

            cleanup_isolation_dirs(config);

            debug!("Isolation child exited with: {:?}", status);
            Ok(IsolationOutput {
                status,
                stdout,
                stderr,
            })
        }
        Err(e) => Err(WrightError::IsolationError(format!("fork: {e}"))),
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
                return Err(WrightError::IsolationError(format!("waitpid: {e}")));
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
            matches!(waitpid(child, None), Ok(WaitStatus::Exited(_, 0)))
        }
        Err(_) => false,
    }
}

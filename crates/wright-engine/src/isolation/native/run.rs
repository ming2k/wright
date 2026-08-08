use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use nix::fcntl::OFlag;
use nix::mount::{MntFlags, MsFlags, mount, umount2};
use nix::sched::{CloneFlags, unshare};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, chdir, execve, fork, getpid, getppid, pivot_root, sethostname};
use tracing::debug;

use crate::isolation::error::{IsolationError, Result};
use crate::isolation::resources::{apply_cpu_affinity, apply_rlimits};
use crate::isolation::{IsolationConfig, IsolationLevel};

use super::exec::prepare_exec;
use super::mounts::prepare_mount_destination;
use super::scratch::{cleanup_isolation_dirs, isolation_scratch_base, prepare_isolation_dirs};

/// Run a command inside native Linux namespace isolation.
///
/// The application process first re-executes Wright's single-threaded helper.
/// This function owns the helper-side double-fork required for a PID
/// namespace:
///
/// ```text
/// Application ──exec──> Single-threaded helper
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
/// Enter namespaces from the dedicated helper process. The helper binary
/// calls this before starting any threads, so all post-fork setup runs from a
/// single-threaded process.
pub(in crate::isolation) fn run_in_helper(
    config: &IsolationConfig,
    command: &str,
    args: &[String],
) -> Result<ExitStatus> {
    if config.level == IsolationLevel::None {
        return Err(IsolationError::InvalidConfig(
            "the isolation helper cannot execute level none".to_string(),
        ));
    }
    run_local(config, command, args)
}

fn run_local(config: &IsolationConfig, command: &str, args: &[String]) -> Result<ExitStatus> {
    config.validate()?;

    let real_uid = nix::unistd::getuid();
    let real_gid = nix::unistd::getgid();

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

    // Always create a user namespace, including when Wright itself runs as
    // root. Capabilities inside that namespace do not confer capabilities in
    // the parent namespace, which is a required part of the sandbox boundary.
    clone_flags |= CloneFlags::CLONE_NEWUSER;

    // Probe whether the required namespaces are available.
    require_namespace_support(config.level, can_unshare(clone_flags))?;

    let (c_command, c_args, c_env) = prepare_exec(config, command, args)?;
    let prepared_limits = config.rlimits.prepare()?;

    // Error pipe: child/grandchild write error messages, parent reads.
    let (err_read, err_write) = nix::unistd::pipe2(OFlag::O_CLOEXEC)
        .map_err(|error| IsolationError::system("create setup-error pipe", error))?;
    // Stdout/stderr pipes: grandchild writes, parent reads + tees.
    let (out_read, out_write) = nix::unistd::pipe2(OFlag::O_CLOEXEC)
        .map_err(|error| IsolationError::system("create stdout pipe", error))?;
    let out_write_fd = out_write.as_raw_fd();
    let (eout_read, eout_write) = nix::unistd::pipe2(OFlag::O_CLOEXEC)
        .map_err(|error| IsolationError::system("create stderr pipe", error))?;
    let eout_write_fd = eout_write.as_raw_fd();

    let scratch = isolation_scratch_base(config);
    prepare_isolation_dirs(&scratch)?;
    let parent_pid = getpid();

    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            drop(err_read);
            drop(out_read);
            drop(eout_read);

            let die = |msg: String| -> ! {
                let bytes = msg.as_bytes();
                let _ = nix::unistd::write(&err_write, bytes);
                unsafe { libc::_exit(1) }
            };

            if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
                die(format!(
                    "set parent-death signal: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if getppid() != parent_pid {
                die("parent exited during isolation setup".to_string());
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
                    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
                        die(format!(
                            "set namespace-init parent-death signal: {}",
                            std::io::Error::last_os_error()
                        ));
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

                    let newroot = scratch.join("root");
                    let upper = scratch.join("upper");
                    let work = scratch.join("work");

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
                                target: &Path,
                                readonly: bool|
                     -> std::result::Result<(), String> {
                        let dest = prepare_mount_destination(&newroot, src, target)?;

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
                    if let Err(e) = bind(&config.src_dir, Path::new("/build"), false) {
                        die(e);
                    }
                    if let Err(e) = bind(&config.output_dir, Path::new("/output"), false) {
                        die(e);
                    }

                    // On merged-/usr systems the lowerdir collapses to a single
                    // directory (e.g. /usr), which overlayfs flattens so that
                    // /usr/lib appears as /lib and there is no /usr directory.
                    // The host's ld.so.cache (bind-mounted below) contains
                    // absolute paths like /usr/lib/..., which would fail to
                    // resolve.  Bind-mount the host /usr to restore the
                    // expected hierarchy.
                    if newroot.join("usr").metadata().is_err()
                        && let Err(e) = bind(Path::new("/usr"), Path::new("/usr"), true)
                    {
                        die(e);
                    }
                    // Extra binds.
                    for (host, dest, ro) in &config.extra_binds {
                        if let Err(e) = bind(host, dest, *ro) {
                            die(e);
                        }
                    }
                    // Build dependency mounts (read-only).
                    for (host, dest) in &config.dep_mounts {
                        if let Err(e) = bind(host, dest, true) {
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
                        if p.exists()
                            && let Err(e) = bind(p, Path::new(etc_file), true)
                        {
                            die(e);
                        }
                    }

                    // --- pivot_root ---

                    let old_root = newroot.join(".old_root");
                    if old_root.symlink_metadata().is_err()
                        && let Err(e) = std::fs::create_dir_all(&old_root)
                    {
                        die(format!("mkdir {}: {e}", old_root.display()));
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

                    // --- chdir + exec ---
                    if let Err(e) = chdir("/build") {
                        die(format!("chdir /build: {e}"));
                    }

                    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
                        die(format!(
                            "set no_new_privs: {}",
                            std::io::Error::last_os_error()
                        ));
                    }

                    // Redirect stdout/stderr to pipes for capture.
                    if unsafe { libc::dup2(out_write_fd, 1) } == -1 {
                        die(format!(
                            "redirect stdout: {}",
                            std::io::Error::last_os_error()
                        ));
                    }
                    if unsafe { libc::dup2(eout_write_fd, 2) } == -1 {
                        die(format!(
                            "redirect stderr: {}",
                            std::io::Error::last_os_error()
                        ));
                    }
                    // Close all pipe fds (originals no longer needed after dup2).
                    drop(out_write);
                    drop(eout_write);

                    // Close error pipe before exec.
                    drop(err_write);

                    // Apply resource limits before exec.
                    if let Err(e) = apply_rlimits(prepared_limits) {
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
                        match execve(&c_command, &c_args, &c_env) {
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
                    drop(out_write);
                    drop(eout_write);
                    drop(err_write);

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
                cleanup_isolation_dirs(&scratch);
                return Err(IsolationError::Setup(msg));
            }

            // The application process owns capture, logging, cancellation and
            // wall-clock timeout policy. This single-threaded helper only
            // forwards the namespace child's two output streams.
            let out_file = std::fs::File::from(out_read);
            let err_file = std::fs::File::from(eout_read);
            let stdout_handle = forward_stream(out_file, ForwardStream::Stdout);
            let stderr_handle = forward_stream(err_file, ForwardStream::Stderr);

            let status = wait_for_child(child);
            let stdout = join_forward("forward isolated stdout", stdout_handle);
            let stderr = join_forward("forward isolated stderr", stderr_handle);

            cleanup_isolation_dirs(&scratch);

            let status = status?;
            stdout?;
            stderr?;

            debug!(
                event = "isolation.child_exited",
                ?status,
                "Isolation child exited"
            );
            Ok(status)
        }
        Err(error) => {
            cleanup_isolation_dirs(&scratch);
            Err(IsolationError::system("fork isolation supervisor", error))
        }
    }
}

#[derive(Clone, Copy)]
enum ForwardStream {
    Stdout,
    Stderr,
}

fn forward_stream<R: Read + Send + 'static>(
    mut source: R,
    stream: ForwardStream,
) -> std::thread::JoinHandle<std::io::Result<()>> {
    std::thread::spawn(move || match stream {
        ForwardStream::Stdout => {
            let mut destination = std::io::stdout().lock();
            std::io::copy(&mut source, &mut destination)?;
            destination.flush()
        }
        ForwardStream::Stderr => {
            let mut destination = std::io::stderr().lock();
            std::io::copy(&mut source, &mut destination)?;
            destination.flush()
        }
    })
}

fn join_forward(
    operation: &'static str,
    handle: std::thread::JoinHandle<std::io::Result<()>>,
) -> Result<()> {
    handle
        .join()
        .map_err(|_| IsolationError::System {
            operation,
            message: "output forwarding thread panicked".to_string(),
        })?
        .map_err(|error| IsolationError::io(operation, error))
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
                return Err(IsolationError::system("wait for isolation process", e));
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

fn require_namespace_support(level: IsolationLevel, available: bool) -> Result<()> {
    if available {
        Ok(())
    } else {
        Err(IsolationError::Unavailable { level })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_modes_fail_closed() {
        let error = require_namespace_support(IsolationLevel::Strict, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("refusing to execute directly on the host"));
    }
}

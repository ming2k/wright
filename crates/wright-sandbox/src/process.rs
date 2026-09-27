//! Application-side child supervision.
//!
//! This is the only isolation module coupled to the engine's process-wide
//! cancellation registry. It owns wall-clock timeouts and output capture for
//! both direct commands and the single-threaded namespace helper.

use std::io::Write;
use std::process::Child;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use super::error::{IsolationError, Result};
use super::{CapturedOutput, IsolationConfig, IsolationOutput, spawn_stream_reader};

pub(super) fn supervise(
    mut child: Child,
    config: &mut IsolationConfig,
    kill_pgroup: bool,
) -> Result<IsolationOutput> {
    let pid = child.id();
    let reap_guard = crate::cancellation::register(pid, kill_pgroup);
    let watchdog = config
        .rlimits
        .timeout_secs
        .map(|timeout| TimeoutWatchdog::spawn(pid, timeout, kill_pgroup));

    let capture = (|| {
        let stdout_capture = capture_file("create stdout capture file")?;
        let stderr_capture = capture_file("create stderr capture file")?;
        let stdout = take_pipe(child.stdout.take(), "capture child stdout")?;
        let stderr = take_pipe(child.stderr.take(), "capture child stderr")?;
        Ok::<_, IsolationError>((stdout_capture, stderr_capture, stdout, stderr))
    })();
    let (stdout_capture, stderr_capture, stdout, stderr) = match capture {
        Ok(capture) => capture,
        Err(error) => {
            terminate(pid, kill_pgroup);
            drop(watchdog);
            drop(reap_guard);
            let _ = child.wait();
            return Err(error);
        }
    };

    let stdout_echo: Option<Box<dyn Write + Send>> = config
        .verbose
        .then(|| Box::new(std::io::stderr()) as Box<dyn Write + Send>);
    let stderr_echo: Option<Box<dyn Write + Send>> = config
        .verbose
        .then(|| Box::new(std::io::stderr()) as Box<dyn Write + Send>);
    let stdout_log = config
        .log_stdout
        .take()
        .map(|file| Box::new(file) as Box<dyn Write + Send>);
    let stderr_log = config
        .log_stderr
        .take()
        .map(|file| Box::new(file) as Box<dyn Write + Send>);
    let stdout_handle = spawn_stream_reader(stdout, stdout_echo, stdout_log, stdout_capture);
    let stderr_handle = spawn_stream_reader(stderr, stderr_echo, stderr_log, stderr_capture);

    let status = child
        .wait()
        .map_err(|error| IsolationError::io("wait for child process", error));
    if status.is_err() {
        terminate(pid, kill_pgroup);
    }

    // Deregister immediately after wait/reap. Keeping a completed PID in
    // either supervisor while capture threads drain could target a reused PID.
    drop(watchdog);
    drop(reap_guard);
    if status.is_err() {
        let _ = child.wait();
    }

    let stdout = join_capture("capture child stdout", stdout_handle);
    let stderr = join_capture("capture child stderr", stderr_handle);

    let status = status?;
    let stdout = stdout?;
    let stderr = stderr?;

    Ok(IsolationOutput {
        status,
        stdout,
        stderr,
    })
}

fn capture_file(operation: &'static str) -> Result<std::fs::File> {
    tempfile::tempfile().map_err(|error| IsolationError::io(operation, error))
}

fn take_pipe<T>(pipe: Option<T>, operation: &'static str) -> Result<T> {
    pipe.ok_or_else(|| IsolationError::System {
        operation,
        message: "child stream was not piped".to_string(),
    })
}

fn join_capture(
    operation: &'static str,
    handle: std::thread::JoinHandle<CapturedOutput>,
) -> Result<CapturedOutput> {
    handle.join().map_err(|_| IsolationError::System {
        operation,
        message: "capture thread panicked".to_string(),
    })
}

fn terminate(pid: u32, kill_pgroup: bool) {
    let target = if kill_pgroup {
        -(pid as i32)
    } else {
        pid as i32
    };
    // SAFETY: `target` identifies either the supervised child or its process
    // group, which is led by that child.
    unsafe {
        libc::kill(target, libc::SIGKILL);
    }
}

struct TimeoutWatchdog {
    cancel: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TimeoutWatchdog {
    fn spawn(pid: u32, timeout: u64, kill_pgroup: bool) -> Self {
        let (cancel, cancelled) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if matches!(
                cancelled.recv_timeout(Duration::from_secs(timeout)),
                Err(RecvTimeoutError::Timeout)
            ) {
                tracing::error!(
                    event = "isolation.timeout",
                    timeout_secs = timeout,
                    pid,
                    kill_pgroup,
                    "Wall-clock timeout exceeded, killing supervised process"
                );
                terminate(pid, kill_pgroup);
            }
        });
        Self {
            cancel: Some(cancel),
            thread: Some(thread),
        }
    }
}

impl Drop for TimeoutWatchdog {
    fn drop(&mut self) {
        self.cancel.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

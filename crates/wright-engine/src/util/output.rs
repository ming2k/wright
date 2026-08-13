//! Process-wide output and SIGPIPE policy.
//!
//! Wright keeps Rust's default `SIGPIPE = SIG_IGN` disposition. A package
//! manager is a multi-threaded process supervisor: libraries write to pipes
//! the application does not even see (e.g. gix writing git protocol to a
//! spawned `ssh`'s stdin), and with the traditional `SIG_DFL` any such write
//! to a dead reader kills the whole process instantly and silently (exit
//! code 141, no logs). With `SIG_IGN` the write instead returns an ordinary
//! `EPIPE` error that the owning operation handles like any other I/O
//! failure. Restoring `SIG_DFL` for `wright list | head`-style semantics is
//! therefore *not* an option; the semantics are implemented here instead:
//!
//! * **stdout is the data channel.** A closed reader is a normal termination
//!   condition — [`outln!`] / [`out!`] exit quietly with code 0 (the
//!   ripgrep/bat convention) rather than panicking like `println!` or dying
//!   of SIGPIPE like a C tool.
//! * **stderr is the diagnostic channel.** Write failures are ignored
//!   outright: diagnostics are best-effort and a closed stderr must never
//!   change control flow ([`errln!`]).
//!
//! The mirror image of this policy lives in [`restore_default_sigpipe`]:
//! processes that exec *user* code (build stages, deploy hooks, interactive
//! shells) get the traditional default dispositions back, because `SIG_IGN`
//! is inherited across `execve` and would otherwise leak into shell
//! pipelines that rely on SIGPIPE death (`tar … | head`).

use std::io::Write;

/// Write a line to stdout; on a closed pipe, exit quietly with code 0.
///
/// Panics on any other write error, matching `println!` semantics.
pub fn stdout_println(args: std::fmt::Arguments<'_>) {
    stdout_write(args, b"\n");
}

/// Write a fragment to stdout; on a closed pipe, exit quietly with code 0.
///
/// Panics on any other write error, matching `print!` semantics.
pub fn stdout_print(args: std::fmt::Arguments<'_>) {
    stdout_write(args, b"");
}

fn stdout_write(args: std::fmt::Arguments<'_>, suffix: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    let result = match args.as_str() {
        Some(s) => stdout.write_all(s.as_bytes()),
        None => stdout.write_fmt(args),
    }
    .and_then(|()| stdout.write_all(suffix))
    .and_then(|()| stdout.flush());
    match result {
        Ok(()) => {}
        // The consumer of our output closed the pipe (`wright list | head`).
        // Nothing can be delivered any more; terminate quietly.
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
            std::process::exit(0);
        }
        Err(error) => panic!("failed printing to stdout: {error}"),
    }
}

/// Write a line to stderr, ignoring all write failures.
pub fn stderr_println(args: std::fmt::Arguments<'_>) {
    stderr_write(args, b"\n");
}

/// Write a fragment to stderr, ignoring all write failures.
pub fn stderr_print(args: std::fmt::Arguments<'_>) {
    stderr_write(args, b"");
}

fn stderr_write(args: std::fmt::Arguments<'_>, suffix: &[u8]) {
    let mut stderr = std::io::stderr().lock();
    let _ = match args.as_str() {
        Some(s) => stderr.write_all(s.as_bytes()),
        None => stderr.write_fmt(args),
    }
    .and_then(|()| stderr.write_all(suffix))
    .and_then(|()| stderr.flush());
}

/// Restore the default SIGPIPE disposition in a process about to exec user
/// code (build stages, deploy hooks, interactive shells).
///
/// `SIG_IGN` is inherited across `execve`; without this reset, shell
/// pipelines inside builds would see `EPIPE` write errors instead of the
/// traditional SIGPIPE death they were written against. This mirrors
/// Python's `subprocess` `restore_signals=True` behavior.
///
/// Safe to call in a post-fork child: `signal` performs a single fixed-size
/// `sigaction` syscall with no allocation.
pub fn restore_default_sigpipe() {
    // SAFETY: setting a disposition is always sound; the caller picks the
    // process it applies to (a child immediately before exec).
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// `println!` for the stdout data channel: writes a line, and exits quietly
/// with code 0 if the reader has gone away (`wright list | head`).
#[macro_export]
macro_rules! outln {
    () => { $crate::util::output::stdout_print(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::util::output::stdout_println(format_args!($($arg)*)) };
}

/// `print!` for the stdout data channel (no trailing newline). See [`outln!`].
#[macro_export]
macro_rules! out {
    ($($arg:tt)*) => { $crate::util::output::stdout_print(format_args!($($arg)*)) };
}

/// `eprintln!` for the stderr diagnostic channel: best-effort, never panics
/// on a closed or broken stderr.
#[macro_export]
macro_rules! errln {
    () => { $crate::util::output::stderr_print(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::util::output::stderr_println(format_args!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn macros_compose_with_format_args() {
        // Compile-level exercise of every arm; output goes to the test
        // harness's captured streams.
        crate::outln!();
        crate::outln!("plain");
        crate::outln!("{} {value}", "positional", value = 42);
        crate::out!("no newline {}", 1);
        crate::errln!();
        crate::errln!("warn {}", "x");
    }
}

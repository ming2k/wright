pub mod checksum;
pub mod display;
pub mod download;
pub mod logging;
pub mod progress;
pub mod stdin;

/// Compact a file path for logging by replacing middle segments with `…`
/// when the path exceeds 45 characters.
pub fn compact_path(path: &str) -> String {
    if path.len() <= 45 {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 4 {
        return path.to_string();
    }
    let n = parts.len();
    format!("/{}/.../{}/{}", parts[0], parts[n - 2], parts[n - 1])
}

/// Strip path separators and dangerous components from a filename derived from a URL.
pub fn sanitize_filename(raw: &str) -> String {
    wright_part::store::sanitize_cache_filename(raw)
}

/// Restore the default SIGPIPE disposition so that writing to a closed
/// stdout/stderr pipe terminates the process quietly — the normal Unix CLI
/// behavior (e.g. `wright list | head`) — instead of panicking inside
/// `println!` with exit code 101. Rust ignores SIGPIPE at startup; call this
/// once at the top of `main` to opt back into the platform default.
#[cfg(unix)]
pub fn reset_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// No-op on non-Unix platforms (see the Unix variant).
#[cfg(not(unix))]
pub fn reset_sigpipe() {}

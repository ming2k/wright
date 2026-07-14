pub mod checksum;
pub mod compress;
pub mod display;
pub mod download;
pub mod lock;
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

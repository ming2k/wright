pub mod checksum;
pub mod compress;
pub mod download;
pub mod lock;
pub mod progress;

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
    let name = raw.rsplit('/').next().unwrap_or(raw);
    let name = name.rsplit('\\').next().unwrap_or(name);
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '\0' {
                '_'
            } else {
                c
            }
        })
        .collect();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "download".to_string()
    } else {
        sanitized
    }
}

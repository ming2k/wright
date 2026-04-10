pub mod checksum;
pub mod compress;
pub mod download;
pub mod lock;
pub mod progress;

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

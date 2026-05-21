use reqwest::blocking::Client;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{Result, WrightError};
use crate::util::compress;
use crate::util::progress;

const MAX_RETRIES: u32 = 3;

/// Download a file from `url` to `dest` atomically.
///
/// For HTTP(S) downloads, the data is first written to a temporary file in the
/// same directory as `dest`, then renamed into place on success. This prevents
/// partial/corrupt files from being left in the cache when a download is
/// interrupted. HTTP(S) downloads are retried up to `MAX_RETRIES` times on
/// transient network errors.
pub fn download_file(url: &str, dest: &Path, timeout: u64, scope: &str) -> Result<()> {
    let label = progress::source_label(url);

    if url.starts_with("file://") {
        let path_str = url.trim_start_matches("file://");
        let src_path = Path::new(path_str);

        if !src_path.exists() {
            return Err(WrightError::NetworkError(format!(
                "local path not found: {}",
                path_str
            )));
        }

        let _span = crate::cli_span!("Fetching", "{} ({})", label, scope);
        if src_path.is_dir() {
            compress::create_tar_zst(src_path, dest)?;
        } else {
            std::fs::copy(src_path, dest).map_err(WrightError::IoError)?;
        }
        return Ok(());
    }

    // Ensure the parent directory exists
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(WrightError::IoError)?;
    }

    let client = Client::builder()
        .user_agent("wright/0.1.0 (Linux; x86_64)")
        .connect_timeout(std::time::Duration::from_secs(timeout))
        .timeout(std::time::Duration::from_secs(timeout))
        .build()
        .map_err(|e| WrightError::NetworkError(format!("failed to create client: {}", e)))?;

    let mut last_err: Option<WrightError> = None;
    for attempt in 1..=MAX_RETRIES {
        match try_download_http(&client, url, dest, &label, scope) {
            Ok(()) => return Ok(()),
            // Fatal errors (e.g. HTTP 404) won't change on retry — fail now.
            Err(Attempt::Fatal(e)) => return Err(e),
            Err(Attempt::Transient(e)) => {
                if attempt < MAX_RETRIES {
                    tracing::warn!("{} ({}/{}); retrying…", e, attempt, MAX_RETRIES);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap())
}

/// Outcome of a single download attempt that failed.
enum Attempt {
    /// Transient (connection drop, timeout, 5xx, rate-limit) — worth a retry.
    Transient(WrightError),
    /// Permanent (4xx, redirect page) — retrying cannot help.
    Fatal(WrightError),
}

fn try_download_http(
    client: &Client,
    url: &str,
    dest: &Path,
    label: &str,
    scope: &str,
) -> std::result::Result<(), Attempt> {
    let mut response = client.get(url).send().map_err(|e| {
        // A failed send is a connection-level problem: usually transient.
        Attempt::Transient(WrightError::NetworkError(format!("cannot reach {}: {}", url, e)))
    })?;

    let status = response.status();
    if !status.is_success() {
        let msg = WrightError::NetworkError(format!("HTTP {} for {}", status, url));
        // Retry only on server errors and rate-limiting; 4xx is permanent.
        return Err(
            if status.is_server_error()
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                Attempt::Transient(msg)
            } else {
                Attempt::Fatal(msg)
            },
        );
    }

    // Reject HTML responses — this usually means the server returned a
    // redirect/mirror-selection page instead of the actual file (common
    // with SourceForge prdownloads URLs).  This won't change on retry.
    if let Some(ct) = response.headers().get(reqwest::header::CONTENT_TYPE)
        && let Ok(ct_str) = ct.to_str()
        && ct_str.contains("text/html")
    {
        return Err(Attempt::Fatal(WrightError::NetworkError(format!(
            "{} returned an HTML page, not a file (likely a redirect; use a direct URL)",
            url
        ))));
    }

    let total_size = response.content_length().unwrap_or(0);
    let span = crate::cli_span!("Fetching", "{} ({})", label, scope);
    progress::record_bytes(&span, 0, total_size);

    // Write to a temporary file in the same directory, then rename on success.
    // Local filesystem failures are fatal — a retry of the network won't help.
    let dest_dir = dest.parent().unwrap_or(Path::new("."));
    let tmp_file = tempfile::NamedTempFile::new_in(dest_dir)
        .map_err(|e| Attempt::Fatal(WrightError::IoError(e)))?;
    let mut file = tmp_file
        .as_file()
        .try_clone()
        .map_err(|e| Attempt::Fatal(WrightError::IoError(e)))?;

    let mut downloaded: u64 = 0;
    let mut buffer = [0; 8192];

    loop {
        // A read failure mid-stream is a dropped connection: transient.
        let n = response.read(&mut buffer).map_err(|e| {
            Attempt::Transient(WrightError::NetworkError(format!(
                "connection interrupted while fetching {}: {}",
                url, e
            )))
        })?;

        if n == 0 {
            break;
        }

        file.write_all(&buffer[..n])
            .map_err(|e| Attempt::Fatal(WrightError::IoError(e)))?;

        downloaded += n as u64;
        progress::record_bytes(&span, downloaded, total_size);
    }

    // Atomically move the completed download into place
    tmp_file
        .persist(dest)
        .map_err(|e| Attempt::Fatal(WrightError::IoError(e.error)))?;

    Ok(())
}

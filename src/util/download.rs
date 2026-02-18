use std::io::{Read, Write};
use std::path::Path;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use tracing::info;

use crate::error::{WrightError, Result};
use crate::util::compress;

/// Download a file from `url` to `dest` atomically.
///
/// For HTTP(S) downloads, the data is first written to a temporary file in the
/// same directory as `dest`, then renamed into place on success. This prevents
/// partial/corrupt files from being left in the cache when a download is
/// interrupted.
pub fn download_file(url: &str, dest: &Path, timeout: u64) -> Result<()> {
    if url.starts_with("file://") {
        let path_str = url.trim_start_matches("file://");
        let src_path = Path::new(path_str);

        if !src_path.exists() {
            return Err(WrightError::NetworkError(format!("local path not found: {}", path_str)));
        }

        if src_path.is_dir() {
            info!("Packing local directory {}...", path_str);
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

    let mut response = client.get(url)
        .send()
        .map_err(|e| WrightError::NetworkError(format!("failed to send request to {}: {}", url, e)))?;

    if !response.status().is_success() {
        return Err(WrightError::NetworkError(format!(
            "failed to download from {}: status {}",
            url, response.status()
        )));
    }

    // Reject HTML responses — this usually means the server returned a
    // redirect/mirror-selection page instead of the actual file (common
    // with SourceForge prdownloads URLs).
    if let Some(ct) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        if let Ok(ct_str) = ct.to_str() {
            if ct_str.contains("text/html") {
                return Err(WrightError::NetworkError(format!(
                    "server returned HTML instead of a file for {} (possible redirect page; \
                     try a direct download URL)",
                    url
                )));
            }
        }
    }

    let total_size = response
        .content_length()
        .unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({eta})")
        .unwrap()
        .progress_chars("#>-"));

    // Write to a temporary file in the same directory, then rename on success.
    let dest_dir = dest.parent().unwrap_or(Path::new("."));
    let tmp_file = tempfile::NamedTempFile::new_in(dest_dir).map_err(WrightError::IoError)?;
    let mut file = tmp_file.as_file().try_clone().map_err(WrightError::IoError)?;

    let mut downloaded: u64 = 0;
    let mut buffer = [0; 8192];

    loop {
        let n = response.read(&mut buffer).map_err(|e| {
            WrightError::IoError(e)
        })?;

        if n == 0 {
            break;
        }

        file.write_all(&buffer[..n]).map_err(|e| {
            WrightError::IoError(e)
        })?;

        downloaded += n as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message("downloaded");

    // Atomically move the completed download into place
    tmp_file.persist(dest).map_err(|e| {
        WrightError::IoError(e.error)
    })?;

    Ok(())
}

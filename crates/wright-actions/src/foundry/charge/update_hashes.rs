use std::path::Path;
use tracing::{debug, info};

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::util::{checksum, download};
use wright_plan::manifest::{PlanManifest, Source};

use super::{Charge, source_cache_filename};

impl Charge {
    // ------------------------------------------------------------------
    // Hash update utility
    // ------------------------------------------------------------------

    pub async fn update_hashes(&self, manifest: &PlanManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();
        if tokio::fs::metadata(&self.cache_dir).await.is_err() {
            tokio::fs::create_dir_all(&self.cache_dir)
                .await
                .map_err(WrightError::IoError)?;
        }
        for source in manifest.sources.entries.iter() {
            match source {
                Source::Http(http) => {
                    let processed_url = variables::process_uri(&http.url, manifest);
                    let cache_filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.metadata.name, &processed_url)
                    });
                    let cache_path = self.cache_dir.join(&cache_filename);
                    if tokio::fs::metadata(&cache_path).await.is_ok() {
                        debug!("Using cached source: {}", cache_filename);
                    } else {
                        info!("Downloading {}...", processed_url);
                        download::download_file(
                            &processed_url,
                            &cache_path,
                            self.download_timeout,
                            &manifest.metadata.name,
                        )?;
                    }
                    let hash = checksum::sha256_file(&cache_path)?;
                    debug!("Computed hash: {}", hash);
                    new_hashes.push(hash);
                }
                Source::Git(_) | Source::Local(_) => {
                    new_hashes.push("SKIP".to_string());
                }
            }
        }
        if new_hashes.is_empty() {
            info!("No sources to update.");
            return Ok(());
        }
        let content = tokio::fs::read_to_string(manifest_path)
            .await
            .map_err(WrightError::IoError)?;
        let has_array_of_tables = content.contains("[[sources]]");
        let new_content = if has_array_of_tables {
            let sha256_re = regex::Regex::new(r#"(?m)^(sha256\s*=\s*)"[^"]*""#).unwrap();
            let mut result = content.clone();
            let mut hash_idx = 0;
            while let Some(m) = sha256_re.find(&result[..]) {
                if hash_idx < new_hashes.len() {
                    let replacement = format!(
                        "{}\"{}\"",
                        &result[m.start()..m.start() + result[m.start()..].find('"').unwrap()],
                        new_hashes[hash_idx]
                    );
                    result = format!(
                        "{}{}{}",
                        &result[..m.start()],
                        replacement,
                        &result[m.end()..]
                    );
                    hash_idx += 1;
                } else {
                    break;
                }
            }
            result
        } else {
            let re = regex::Regex::new(r"(?m)^sha256\s*=\s*\[[\s\S]*?\]").unwrap();
            let hashes_str = new_hashes
                .iter()
                .map(|h| format!("    \"{h}\""))
                .collect::<Vec<_>>()
                .join(",\n");
            let replacement = format!("sha256 = [\n{},\n]", hashes_str);
            if re.is_match(&content) {
                re.replace(&content, &replacement).to_string()
            } else {
                let uris_re = regex::Regex::new(r"(?m)^uris\s*=\s*\[[\s\S]*?\]").unwrap();
                if uris_re.is_match(&content) {
                    let uris_match = uris_re.find(&content).unwrap();
                    let mut c = content.clone();
                    c.insert_str(uris_match.end(), &format!("\n{replacement}"));
                    c
                } else {
                    return Err(WrightError::ForgeError(
                        "could not find sources or sha256 field in plan.toml".to_string(),
                    ));
                }
            }
        };
        tokio::fs::write(manifest_path, new_content)
            .await
            .map_err(WrightError::IoError)?;
        Ok(())
    }
}

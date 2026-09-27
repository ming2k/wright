use tracing::debug;

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::util::checksum;
use wright_plan::manifest::{PlanManifest, Source};

use super::{Charge, source_cache_filename};

impl Charge {
    // ------------------------------------------------------------------
    // Verify
    // ------------------------------------------------------------------

    pub(super) async fn verify(&self, manifest: &PlanManifest) -> Result<()> {
        for (i, source) in manifest.sources.entries.iter().enumerate() {
            let http = match source {
                Source::Http(h) => h,
                _ => {
                    debug!("Skipping verification for non-HTTP source {}", i);
                    continue;
                }
            };
            if http.sha256 == "SKIP" {
                debug!("Skipping verification for HTTP source {} (SKIP)", i);
                continue;
            }
            let processed_url = variables::process_uri(&http.url, manifest);
            let filename = http
                .r#as
                .clone()
                .unwrap_or_else(|| source_cache_filename(&manifest.metadata.name, &processed_url));
            let path = self.cache_dir.join(&filename);
            if tokio::fs::metadata(&path).await.is_err() {
                return Err(WrightError::ValidationError(format!(
                    "source file missing: {filename}"
                )));
            }
            let actual_hash = checksum::sha256_file(&path)?;
            if actual_hash != http.sha256 {
                return Err(WrightError::ValidationError(format!(
                    "SHA256 mismatch for {filename}:\n  expected: {}\n  actual:   {}",
                    http.sha256, actual_hash
                )));
            }
            debug!("Verified source: {}", filename);
        }
        Ok(())
    }
}

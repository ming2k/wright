use std::path::Path;
use tracing::{debug, warn};

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::util::{checksum, download, progress};
use wright_plan::manifest::{PlanManifest, Source};

use super::Charge;
use super::git::git_snapshot_filename;
use super::identity::resolve_local_path;

impl Charge {
    // ------------------------------------------------------------------
    // Fetch
    // ------------------------------------------------------------------

    pub(super) async fn fetch(&self, manifest: &PlanManifest, plan_dir: &Path) -> Result<()> {
        if tokio::fs::metadata(&self.cache_dir).await.is_err() {
            tokio::fs::create_dir_all(&self.cache_dir)
                .await
                .map_err(WrightError::IoError)?;
        }

        let futs = manifest
            .sources
            .entries
            .iter()
            .map(|source| self.fetch_one(manifest, plan_dir, source));
        futures_util::future::try_join_all(futs).await?;
        Ok(())
    }

    async fn fetch_one(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        source: &Source,
    ) -> Result<()> {
        match source {
            Source::Git(git) => {
                let processed_url = variables::process_uri(&git.url, manifest);
                let processed_ref = git
                    .r#ref
                    .as_deref()
                    .map(|r| variables::process_uri(r, manifest))
                    .unwrap_or_else(|| "HEAD".to_string());
                if git.git_metadata {
                    // Cloned straight into the work directory at extract
                    // time; nothing is cached.
                    return Ok(());
                }
                let filename =
                    git_snapshot_filename(&processed_url, &processed_ref, git.submodules);
                let dest = self.cache_dir.join(&filename);
                // A cold build re-resolves the ref even when a snapshot is
                // present. Sibling tasks in the same cold run may race here;
                // each fetch publishes atomically, so last-writer-wins is
                // safe and the content is equally fresh.
                if self.no_cache || tokio::fs::metadata(&dest).await.is_err() {
                    let _permit = self
                        .network_pool
                        .acquire()
                        .await
                        .expect("network semaphore closed");
                    let charge = self.clone();
                    let url = processed_url.clone();
                    let r#ref = processed_ref.clone();
                    let dest_owned = dest.clone();
                    let scope = manifest.metadata.name.clone();
                    let submodules = git.submodules;
                    let timeout_secs = self.download_timeout;
                    let timeout_dur = std::time::Duration::from_secs(timeout_secs.max(1));
                    let join = tokio::task::spawn_blocking(move || {
                        charge.fetch_git_snapshot(
                            &url,
                            Some(r#ref.as_str()),
                            &dest_owned,
                            &scope,
                            submodules,
                        )
                    });
                    let res = match tokio::time::timeout(timeout_dur, join).await {
                        Ok(join_res) => {
                            join_res.map_err(|e| WrightError::context("git fetch join", e))??
                        }
                        Err(_) => {
                            return Err(WrightError::NetworkError(format!(
                                "timed out after {timeout_secs}s while fetching git repository '{processed_url}'\n  \
                                 If this is a private repository requiring authentication, verify your credentials and network connection."
                            )));
                        }
                    };
                    if let Some(commit_id) = res {
                        debug!("Fetched Git commit: {} for {}", commit_id, filename);
                    }
                }
            }
            Source::Http(http) => {
                let processed_url = variables::process_uri(&http.url, manifest);
                let dest = self
                    .cache_path_for(manifest, source)
                    .expect("http sources are always cached");
                let filename = dest
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let skip_verify = http.sha256 == "SKIP";
                let mut needs_download = true;

                // A declared sha256 proves the bytes: a cold build reuses a
                // matching artefact instead of re-downloading identical
                // content. Only unverifiable sources (`SKIP`) are forced fresh.
                if tokio::fs::metadata(&dest).await.is_ok() {
                    if skip_verify {
                        if self.no_cache {
                            debug!(
                                "Source {} is unpinned and cold mode is active; re-downloading",
                                filename
                            );
                        } else {
                            debug!("Source {} already cached (SKIP verification)", filename);
                            needs_download = false;
                        }
                    } else if let Ok(actual_hash) = checksum::sha256_file(&dest) {
                        if actual_hash == http.sha256 {
                            debug!("Source {} already cached and verified", filename);
                            needs_download = false;
                        } else {
                            warn!(
                                "Cached source {} hash mismatch, re-downloading...",
                                filename
                            );
                            let _ = tokio::fs::remove_file(&dest).await;
                        }
                    }
                }

                if needs_download {
                    let _permit = self
                        .network_pool
                        .acquire()
                        .await
                        .expect("network semaphore closed");
                    let url = processed_url.clone();
                    let dest_owned = dest.clone();
                    let timeout = self.download_timeout;
                    let scope = manifest.metadata.name.clone();
                    tokio::task::spawn_blocking(move || {
                        download::download_file(&url, &dest_owned, timeout, &scope)
                    })
                    .await
                    .map_err(|e| WrightError::context("download join", e))??;
                    if !skip_verify {
                        let actual_hash = checksum::sha256_file(&dest)?;
                        if actual_hash != http.sha256 {
                            return Err(WrightError::ValidationError(format!(
                                "Downloaded file {} failed verification!\n  Expected: {}\n  Actual:   {}",
                                filename, http.sha256, actual_hash
                            )));
                        }
                    }
                }
            }
            Source::Local(local) => {
                let processed_path = variables::process_uri(&local.path, manifest);
                let local_path = resolve_local_path(plan_dir, &processed_path)?;
                let dest = self
                    .cache_path_for(manifest, source)
                    .expect("local sources are always cached");
                let label = progress::source_label(&processed_path);
                let _span = crate::cli_span!("Fetching", "{} ({})", label, manifest.metadata.name);
                tokio::fs::copy(&local_path, &dest).await.map_err(|e| {
                    WrightError::context(
                        format!(
                            "failed to copy local file {} to cache",
                            local_path.display()
                        ),
                        e,
                    )
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalConfig;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    fn make_test_manifest(git_url: &str) -> PlanManifest {
        let toml_str = format!(
            r#"
name = "test-pkg"
version = "1.0.0"
release = 1
description = "test git source"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "git"
url = "{git_url}"
ref = "main"
"#
        );
        PlanManifest::parse(&toml_str).expect("valid manifest")
    }

    fn http_manifest(url: &str, sha256: &str) -> PlanManifest {
        PlanManifest::parse(&format!(
            r#"
name = "demo"
version = "1.0.0"
release = 1
description = "test http source"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "{url}"
sha256 = "{sha256}"
"#
        ))
        .expect("valid manifest")
    }

    fn sha256(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    fn serve(payload: Vec<u8>) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&payload);
            }
        });
        (port, hits)
    }

    #[tokio::test]
    async fn test_charge_fetch_private_git_fails_with_auth_error() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let resp = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"Git\"\r\nContent-Length: 0\r\n\r\n";
                    let _ = stream.write_all(resp.as_bytes());
                }
            }
        });

        let tmp = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = tmp.path().join("sources");
        config.network.download_timeout = 2;
        let pool = Arc::new(Semaphore::new(1));
        let charge = Charge::new(&config, pool);

        let manifest = make_test_manifest(&format!("http://127.0.0.1:{port}/private.git"));
        let result = charge.fetch(&manifest, tmp.path()).await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("authentication required or access denied"),
            "expected auth error, got: {err_msg}"
        );
        assert!(
            err_msg.contains("private repository"),
            "expected private repo note, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn test_charge_fetch_git_timeout() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(_stream) = stream {
                    // Hold connection open to trigger timeout
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
            }
        });

        let tmp = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = tmp.path().join("sources");
        config.network.download_timeout = 1; // 1 second timeout
        let pool = Arc::new(Semaphore::new(1));
        let charge = Charge::new(&config, pool);

        let manifest = make_test_manifest(&format!("http://127.0.0.1:{port}/stalled.git"));
        let start = std::time::Instant::now();
        let result = charge.fetch(&manifest, tmp.path()).await;
        let elapsed = start.elapsed();

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("timed out after 1s"),
            "expected 1s timeout, got: {err_msg}"
        );
        assert!(
            elapsed >= std::time::Duration::from_secs(1),
            "timed out too quickly: {:?}",
            elapsed
        );
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "timed out too slowly: {:?}",
            elapsed
        );
    }

    /// A declared sha256 proves the bytes: cold mode must reuse the file rather
    /// than re-downloading identical content. The URL is unreachable, so any
    /// download attempt fails the test.
    #[tokio::test]
    async fn cold_fetch_reuses_a_verified_pinned_source() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("sources");
        std::fs::create_dir_all(&config.general.source_dir).unwrap();

        let payload = b"pinned bytes";
        let cache = config.general.source_dir.join("demo-data.bin");
        std::fs::write(&cache, payload).unwrap();

        let manifest = http_manifest("http://127.0.0.1:1/data.bin", &sha256(payload));
        let charge = Charge::with_no_cache(&config, Arc::new(Semaphore::new(1)), true);
        charge
            .fetch(&manifest, root.path())
            .await
            .expect("a verified pinned source must not be re-downloaded");
        assert_eq!(std::fs::read(&cache).unwrap(), payload);
    }

    #[tokio::test]
    async fn cold_fetch_redownloads_a_corrupt_pinned_source() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("sources");
        std::fs::create_dir_all(&config.general.source_dir).unwrap();

        let cache = config.general.source_dir.join("demo-data.bin");
        std::fs::write(&cache, b"tampered").unwrap();

        let payload = b"correct bytes".to_vec();
        let (port, hits) = serve(payload.clone());
        let manifest = http_manifest(
            &format!("http://127.0.0.1:{port}/data.bin"),
            &sha256(&payload),
        );
        let charge = Charge::with_no_cache(&config, Arc::new(Semaphore::new(1)), true);
        charge.fetch(&manifest, root.path()).await.unwrap();

        assert_eq!(std::fs::read(&cache).unwrap(), payload);
        assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn cold_fetch_always_downloads_an_unpinned_source() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("sources");
        std::fs::create_dir_all(&config.general.source_dir).unwrap();

        let cache = config.general.source_dir.join("demo-data.bin");
        std::fs::write(&cache, b"stale").unwrap();

        let payload = b"fresh bytes".to_vec();
        let (port, hits) = serve(payload.clone());
        let manifest = http_manifest(&format!("http://127.0.0.1:{port}/data.bin"), "SKIP");
        let charge = Charge::with_no_cache(&config, Arc::new(Semaphore::new(1)), true);
        charge.fetch(&manifest, root.path()).await.unwrap();

        assert_eq!(std::fs::read(&cache).unwrap(), payload);
        assert!(hits.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }
}

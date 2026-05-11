use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};

/// Default stage order used for checkpoint rewind.
pub const PIPELINE_STAGE_ORDER: &[&str] = &[
    "fetch",
    "verify",
    "extract",
    "prepare",
    "configure",
    "compile",
    "check",
    "staging",
];

/// The persistent state machine written to `.wright-pipeline.json`.
///
/// Records the completion status and input fingerprint for every lifecycle
/// stage, forming a hash chain that detects configuration changes anywhere
/// upstream and cascades invalidation to all downstream stages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineState {
    pub plan_name: String,
    pub version: String,
    pub stages: BTreeMap<String, StageRecord>,
}

/// A single stage's execution record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRecord {
    pub status: StageStatus,
    /// Hash of: stage script + stage env vars + previous stage's input_hash.
    /// Empty string if PENDING.
    #[serde(default)]
    pub input_hash: String,
    /// ISO-8601 timestamp of completion (only for COMPLETED).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Error message (only for FAILED).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum StageStatus {
    Pending,
    Completed,
    #[serde(rename = "FAILED")]
    Failed,
}

/// Manages `.wright-pipeline.json` — reading, writing, verifying, and
/// computing smart rewind points for the hash-chain checkpoint system.
pub struct ForgeCheckpoint {
    state_path: PathBuf,
    state: PipelineState,
    work_dir: PathBuf,
}

impl ForgeCheckpoint {
    /// Load an existing pipeline state from `work_dir` or create a fresh one.
    pub fn load(work_dir: PathBuf, plan_name: &str, version: &str) -> Result<Self> {
        let state_path = work_dir.join(".wright-pipeline.json");
        let state = if state_path.exists() {
            let raw = std::fs::read_to_string(&state_path).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to read pipeline state {}: {}",
                    state_path.display(),
                    e
                ))
            })?;
            serde_json::from_str::<PipelineState>(&raw).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to parse pipeline state {}: {}",
                    state_path.display(),
                    e
                ))
            })?
        } else {
            PipelineState {
                plan_name: plan_name.to_string(),
                version: version.to_string(),
                stages: BTreeMap::new(),
            }
        };
        Ok(Self {
            state_path,
            state,
            work_dir,
        })
    }

    /// Persist the current state to disk.
    fn save(&self) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to create parent dir for {}: {}",
                    self.state_path.display(),
                    e
                ))
            })?;
        }
        let raw = serde_json::to_string_pretty(&self.state).map_err(|e| {
            WrightError::ForgeError(format!("failed to serialize pipeline state: {}", e))
        })?;

        let tmp_path = self.state_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, raw).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to write temporary pipeline state {}: {}",
                tmp_path.display(),
                e
            ))
        })?;

        std::fs::rename(&tmp_path, &self.state_path).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to atomicity commit pipeline state to {}: {}",
                self.state_path.display(),
                e
            ))
        })?;

        Ok(())
    }

    // --- Hash chain computation ---

    /// Compute `input_hash` for a single stage given its script, environment,
    /// and the hash of the preceding stage (empty string for the first stage).
    pub fn compute_input_hash(
        stage_script: &str,
        stage_env: &std::collections::HashMap<String, String>,
        prev_hash: &str,
    ) -> String {
        let mut h = Sha256::new();
        h.update(stage_script.as_bytes());

        // Stable-order env entries.
        let mut keys: Vec<&String> = stage_env.keys().collect();
        keys.sort();
        for k in keys {
            if let Some(v) = stage_env.get(k) {
                h.update(k.as_bytes());
                h.update(b"=");
                h.update(v.as_bytes());
                h.update(b"\n");
            }
        }
        // Chain to previous stage hash for cascade invalidation.
        h.update(prev_hash.as_bytes());
        format!("{:x}", h.finalize())
    }

    /// Compute the full vector of expected input hashes for every stage in
    /// `stage_order`. Returns a map of stage_name -> expected_hash.
    ///
    /// `stage_config` provides (script, env) for each stage.
    pub fn compute_expected_hashes(
        stage_order: &[String],
        stage_config: &std::collections::HashMap<
            String,
            (String, std::collections::HashMap<String, String>),
        >,
    ) -> std::collections::HashMap<String, String> {
        let mut results = std::collections::HashMap::new();
        let mut prev_hash = String::new();
        for name in stage_order {
            if let Some((script, env)) = stage_config.get(name) {
                let h = Self::compute_input_hash(script, env, &prev_hash);
                results.insert(name.clone(), h.clone());
                prev_hash = h;
            }
        }
        results
    }

    // --- Smart resume: find the first stage that needs re-execution ---

    /// Find the rewind point: the index into `stage_order` of the first stage
    /// whose stored `input_hash` does not match `expected_hashes`, or whose
    /// stored status is not `Completed`. Returns `None` when every stage is up
    /// to date.
    pub fn find_rewind_point(
        &self,
        stage_order: &[String],
        expected_hashes: &std::collections::HashMap<String, String>,
    ) -> Option<usize> {
        for (idx, name) in stage_order.iter().enumerate() {
            let record = self.state.stages.get(name);
            let stored_hash = record.map(|r| r.input_hash.as_str()).unwrap_or("");
            let stored_status = record.map(|r| r.status).unwrap_or(StageStatus::Pending);

            if stored_status != StageStatus::Completed {
                debug!(
                    "Rewind point at stage '{}' (index {}): status is {:?}",
                    name, idx, stored_status
                );
                return Some(idx);
            }

            let expected = expected_hashes.get(name).map(|s| s.as_str()).unwrap_or("");
            if stored_hash != expected {
                debug!(
                    "Rewind point at stage '{}' (index {}): hash mismatch (stored={}, expected={})",
                    name,
                    idx,
                    &stored_hash[..16.min(stored_hash.len())],
                    &expected[..16.min(expected.len())]
                );
                return Some(idx);
            }
        }
        None
    }

    // --- State mutation ---

    /// Mark a stage as completed with its input hash.
    pub fn mark_complete(&mut self, stage: &str, input_hash: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.state.stages.insert(
            stage.to_string(),
            StageRecord {
                status: StageStatus::Completed,
                input_hash: input_hash.to_string(),
                completed_at: Some(now),
                error: None,
            },
        );
        self.save()
    }

    /// Mark a stage as failed with an error message.
    pub fn mark_failed(&mut self, stage: &str, input_hash: &str, error: &str) -> Result<()> {
        self.state.stages.insert(
            stage.to_string(),
            StageRecord {
                status: StageStatus::Failed,
                input_hash: input_hash.to_string(),
                completed_at: None,
                error: Some(error.to_string()),
            },
        );
        self.save()
    }

    /// Rewind: reset stages from `from_idx` forward in `stage_order`.
    /// Both the JSON records and on-disk layer directories are cleared.
    pub fn rewind_from(&mut self, stage_order: &[String], from_idx: usize) -> Result<()> {
        info!(
            "Rewinding pipeline from stage '{}' (index {})",
            stage_order[from_idx], from_idx
        );

        // Reset JSON state for stage N and beyond.
        for name in &stage_order[from_idx..] {
            self.state.stages.insert(
                name.clone(),
                StageRecord {
                    status: StageStatus::Pending,
                    input_hash: String::new(),
                    completed_at: None,
                    error: None,
                },
            );

            // Clear the on-disk layer directory.
            let layer_dir = self
                .work_dir
                .join("layers")
                .join(crate::forge::layers::layer_dir_name(name));
            if layer_dir.exists() {
                debug!("Clearing layer directory: {}", layer_dir.display());
                if let Err(e) = std::fs::remove_dir_all(&layer_dir) {
                    warn!("Failed to clear layer {}: {}", layer_dir.display(), e);
                }
            }
        }

        self.save()
    }

    /// Check whether a specific stage is completed (stored hash present
    /// and matches the expected hash).
    pub fn is_complete(&self, stage: &str, expected_hash: &str) -> bool {
        match self.state.stages.get(stage) {
            Some(rec) if rec.status == StageStatus::Completed => rec.input_hash == expected_hash,
            _ => false,
        }
    }

    /// Remove all state — JSON file.
    pub fn invalidate_all(&mut self) {
        let _ = std::fs::remove_file(&self.state_path);
        self.state.stages.clear();
    }

    /// Return a reference to the internal pipeline state.
    pub fn state(&self) -> &PipelineState {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_hash_chain_cascade() {
        let env: HashMap<String, String> = HashMap::new();
        let h1 = ForgeCheckpoint::compute_input_hash("script_a", &env, "");
        let h2 = ForgeCheckpoint::compute_input_hash("script_b", &env, &h1);
        let h2_alt = ForgeCheckpoint::compute_input_hash("script_b_changed", &env, &h1);

        // Changing a script changes its own hash.
        assert_ne!(h2, h2_alt);
        // Different stages should have different hashes even with same script.
        let h1_alt = ForgeCheckpoint::compute_input_hash("script_b", &env, "");
        assert_ne!(h1, h1_alt);
    }

    #[test]
    fn test_hash_env_sensitivity() {
        let mut env_a: HashMap<String, String> = HashMap::new();
        env_a.insert("CFLAGS".to_string(), "-O2".to_string());
        let mut env_b: HashMap<String, String> = HashMap::new();
        env_b.insert("CFLAGS".to_string(), "-O3".to_string());

        let ha = ForgeCheckpoint::compute_input_hash("make", &env_a, "");
        let hb = ForgeCheckpoint::compute_input_hash("make", &env_b, "");
        assert_ne!(ha, hb);
    }

    #[test]
    fn test_find_rewind_point_all_completed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ck = ForgeCheckpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();

        let order: Vec<String> = vec!["fetch".into(), "extract".into()];
        let mut config: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config.insert("fetch".into(), ("fetch_script".into(), HashMap::new()));
        config.insert("extract".into(), ("extract_script".into(), HashMap::new()));

        let expected = ForgeCheckpoint::compute_expected_hashes(&order, &config);
        let fetch_hash = expected.get("fetch").unwrap();
        let extract_hash = expected.get("extract").unwrap();

        ck.mark_complete("fetch", fetch_hash).unwrap();
        ck.mark_complete("extract", extract_hash).unwrap();

        // Reload from disk.
        let ck2 = ForgeCheckpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();
        let pt = ck2.find_rewind_point(&order, &expected);
        assert!(pt.is_none(), "expected no rewind point, got {:?}", pt);
    }

    #[test]
    fn test_find_rewind_point_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ck = ForgeCheckpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();

        let order: Vec<String> = vec!["fetch".into(), "extract".into()];
        let mut config: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config.insert("fetch".into(), ("fetch_script".into(), HashMap::new()));
        config.insert("extract".into(), ("extract_script".into(), HashMap::new()));

        let expected = ForgeCheckpoint::compute_expected_hashes(&order, &config);
        let fetch_hash = expected.get("fetch").unwrap();

        ck.mark_complete("fetch", fetch_hash).unwrap();

        // Now change the expected hashes (simulating a config change).
        let mut config2: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config2.insert("fetch".into(), ("fetch_script".into(), HashMap::new()));
        config2.insert(
            "extract".into(),
            ("extract_script_changed".into(), HashMap::new()),
        );
        let expected2 = ForgeCheckpoint::compute_expected_hashes(&order, &config2);

        let pt = ck.find_rewind_point(&order, &expected2);
        // extract should be the rewind point because its script changed.
        assert_eq!(pt, Some(1));
    }

    #[test]
    fn test_rewind_from() {
        let tmp = tempfile::tempdir().unwrap();
        let layers_dir = tmp.path().join("layers");
        // Use layer_name_for_stage to create directories matching the actual
        // naming scheme used by rewind_from.
        for stage in &["fetch", "extract", "prepare"] {
            let name = crate::forge::layers::layer_dir_name(stage);
            std::fs::create_dir_all(layers_dir.join(&name)).unwrap();
        }

        let mut ck = ForgeCheckpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();
        let order: Vec<String> = vec!["fetch".into(), "extract".into(), "prepare".into()];
        let mut config: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config.insert("fetch".into(), ("s1".into(), HashMap::new()));
        config.insert("extract".into(), ("s2".into(), HashMap::new()));
        config.insert("prepare".into(), ("s3".into(), HashMap::new()));
        let expected = ForgeCheckpoint::compute_expected_hashes(&order, &config);

        // Mark all as complete.
        for n in &order {
            ck.mark_complete(n, expected.get(n).unwrap()).unwrap();
        }

        // Rewind from index 1 (extract).
        ck.rewind_from(&order, 1).unwrap();

        assert!(ck.is_complete("fetch", expected.get("fetch").unwrap()));
        assert!(!ck.is_complete("extract", expected.get("extract").unwrap()));
        assert!(!ck.is_complete("prepare", expected.get("prepare").unwrap()));

        // Layer dirs for extract and prepare should be gone.
        assert!(!layers_dir.join("02-extract").exists());
        assert!(!layers_dir.join("03-prepare").exists());
    }

    #[test]
    fn test_invalidate_all_preserves_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let layers_dir = tmp.path().join("layers");
        std::fs::create_dir_all(&layers_dir).unwrap();
        let fetch_layer = layers_dir.join("01-fetch");
        std::fs::create_dir_all(&fetch_layer).unwrap();

        let mut ck = ForgeCheckpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();
        let state_path = tmp.path().join(".wright-pipeline.json");
        std::fs::write(&state_path, "{}").unwrap();

        assert!(state_path.exists());
        assert!(fetch_layer.exists());

        ck.invalidate_all();

        assert!(!state_path.exists());
        assert!(fetch_layer.exists(), "layers directory should be preserved");
    }
}

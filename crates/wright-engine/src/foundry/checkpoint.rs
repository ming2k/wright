use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};

/// Build stage order for checkpoint rewind. Source stages are NOT included.
pub const STAGE_ORDER: &[&str] = &["prepare", "configure", "compile", "check", "staging"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeState {
    pub plan_name: String,
    pub version: String,
    pub stages: BTreeMap<String, StageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRecord {
    pub status: StageStatus,
    #[serde(default)]
    pub input_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
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

/// Manages `.wright-checkpoint.json` — reading, writing, verifying, and
/// computing smart rewind points for the hash-chain checkpoint system.
pub struct Checkpoint {
    state_path: PathBuf,
    state: ForgeState,
    work_dir: PathBuf,
}

impl Checkpoint {
    pub fn load(work_dir: PathBuf, plan_name: &str, version: &str) -> Result<Self> {
        let state_path = work_dir.join(".wright-checkpoint.json");
        let state = if state_path.exists() {
            let raw = std::fs::read_to_string(&state_path).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to read checkpoint state {}: {e}",
                    state_path.display()
                ))
            })?;
            serde_json::from_str::<ForgeState>(&raw).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to parse checkpoint state {}: {e}",
                    state_path.display()
                ))
            })?
        } else {
            ForgeState {
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

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to create parent dir for {}: {e}",
                    self.state_path.display()
                ))
            })?;
        }
        let raw = serde_json::to_string_pretty(&self.state).map_err(|e| {
            WrightError::ForgeError(format!("failed to serialize checkpoint state: {e}"))
        })?;

        let tmp_path = self.state_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, raw).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to write temporary checkpoint state {}: {e}",
                tmp_path.display()
            ))
        })?;

        std::fs::rename(&tmp_path, &self.state_path).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to atomicity commit checkpoint state to {}: {e}",
                self.state_path.display()
            ))
        })?;

        Ok(())
    }

    pub fn compute_input_hash(
        stage_script: &str,
        stage_env: &std::collections::HashMap<String, String>,
        prev_hash: &str,
    ) -> String {
        let mut h = Sha256::new();
        h.update(stage_script.as_bytes());

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
        h.update(prev_hash.as_bytes());
        format!("{:x}", h.finalize())
    }

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
                    event = "checkpoint.rewind_status",
                    stage = %name,
                    index = idx,
                    ?stored_status,
                    "Rewind point: stage not completed"
                );
                return Some(idx);
            }

            let expected = expected_hashes.get(name).map(|s| s.as_str()).unwrap_or("");
            if stored_hash != expected {
                debug!(
                    event = "checkpoint.rewind_hash",
                    stage = %name,
                    index = idx,
                    stored_hash = %&stored_hash[..16.min(stored_hash.len())],
                    expected_hash = %&expected[..16.min(expected.len())],
                    "Rewind point: hash mismatch"
                );
                return Some(idx);
            }
        }
        None
    }

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

    pub fn rewind_from(&mut self, stage_order: &[String], from_idx: usize) -> Result<()> {
        info!(
            event = "checkpoint.rewind",
            stage = %stage_order[from_idx],
            index = from_idx,
            "Rewinding forge"
        );

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

            let layer_dir = self
                .work_dir
                .join("layers")
                .join(crate::foundry::layers::layer_dir_name(name));
            if layer_dir.exists() {
                debug!(event = "checkpoint.layer_clear", dir = %layer_dir.display(), "Clearing layer directory");
                if let Err(e) = std::fs::remove_dir_all(&layer_dir) {
                    warn!(event = "checkpoint.layer_clear_failed", dir = %layer_dir.display(), error = %e, "Failed to clear layer");
                }
            }
        }

        self.save()
    }

    pub fn is_complete(&self, stage: &str, expected_hash: &str) -> bool {
        match self.state.stages.get(stage) {
            Some(rec) if rec.status == StageStatus::Completed => rec.input_hash == expected_hash,
            _ => false,
        }
    }

    pub fn invalidate_all(&mut self) {
        let _ = std::fs::remove_file(&self.state_path);
        self.state.stages.clear();
    }

    pub fn state(&self) -> &ForgeState {
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
        let h1 = Checkpoint::compute_input_hash("script_a", &env, "");
        let h2 = Checkpoint::compute_input_hash("script_b", &env, &h1);
        let h2_alt = Checkpoint::compute_input_hash("script_b_changed", &env, &h1);
        assert_ne!(h2, h2_alt);
        let h1_alt = Checkpoint::compute_input_hash("script_b", &env, "");
        assert_ne!(h1, h1_alt);
    }

    #[test]
    fn test_hash_env_sensitivity() {
        let mut env_a: HashMap<String, String> = HashMap::new();
        env_a.insert("CFLAGS".to_string(), "-O2".to_string());
        let mut env_b: HashMap<String, String> = HashMap::new();
        env_b.insert("CFLAGS".to_string(), "-O3".to_string());

        let ha = Checkpoint::compute_input_hash("make", &env_a, "");
        let hb = Checkpoint::compute_input_hash("make", &env_b, "");
        assert_ne!(ha, hb);
    }

    #[test]
    fn test_find_rewind_point_all_completed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ck = Checkpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();

        let order: Vec<String> = vec!["prepare".into(), "compile".into()];
        let mut config: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config.insert("prepare".into(), ("prep_script".into(), HashMap::new()));
        config.insert("compile".into(), ("compile_script".into(), HashMap::new()));

        let expected = Checkpoint::compute_expected_hashes(&order, &config);
        let prep_hash = expected.get("prepare").unwrap();
        let compile_hash = expected.get("compile").unwrap();

        ck.mark_complete("prepare", prep_hash).unwrap();
        ck.mark_complete("compile", compile_hash).unwrap();

        let ck2 = Checkpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();
        let pt = ck2.find_rewind_point(&order, &expected);
        assert!(pt.is_none(), "expected no rewind point, got {:?}", pt);
    }

    #[test]
    fn test_find_rewind_point_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ck = Checkpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();

        let order: Vec<String> = vec!["prepare".into(), "compile".into()];
        let mut config: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config.insert("prepare".into(), ("prep_script".into(), HashMap::new()));
        config.insert("compile".into(), ("compile_script".into(), HashMap::new()));

        let expected = Checkpoint::compute_expected_hashes(&order, &config);
        let prep_hash = expected.get("prepare").unwrap();
        ck.mark_complete("prepare", prep_hash).unwrap();

        let mut config2: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config2.insert("prepare".into(), ("prep_script".into(), HashMap::new()));
        config2.insert(
            "compile".into(),
            ("compile_script_changed".into(), HashMap::new()),
        );
        let expected2 = Checkpoint::compute_expected_hashes(&order, &config2);

        let pt = ck.find_rewind_point(&order, &expected2);
        assert_eq!(pt, Some(1));
    }

    #[test]
    fn test_rewind_from() {
        let tmp = tempfile::tempdir().unwrap();
        let layers_dir = tmp.path().join("layers");
        for stage in &["prepare", "compile", "staging"] {
            let name = crate::foundry::layers::layer_dir_name(stage);
            std::fs::create_dir_all(layers_dir.join(&name)).unwrap();
        }

        let mut ck = Checkpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();
        let order: Vec<String> = vec!["prepare".into(), "compile".into(), "staging".into()];
        let mut config: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
        config.insert("prepare".into(), ("s1".into(), HashMap::new()));
        config.insert("compile".into(), ("s2".into(), HashMap::new()));
        config.insert("staging".into(), ("s3".into(), HashMap::new()));
        let expected = Checkpoint::compute_expected_hashes(&order, &config);

        for n in &order {
            ck.mark_complete(n, expected.get(n).unwrap()).unwrap();
        }

        ck.rewind_from(&order, 1).unwrap();

        assert!(ck.is_complete("prepare", expected.get("prepare").unwrap()));
        assert!(!ck.is_complete("compile", expected.get("compile").unwrap()));
        assert!(!ck.is_complete("staging", expected.get("staging").unwrap()));

        assert!(!layers_dir.join("02-compile").exists());
        assert!(!layers_dir.join("05-staging").exists());
    }

    #[test]
    fn test_invalidate_all_preserves_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let layers_dir = tmp.path().join("layers");
        std::fs::create_dir_all(&layers_dir).unwrap();
        let prepare_layer = layers_dir.join("01-prepare");
        std::fs::create_dir_all(&prepare_layer).unwrap();

        let mut ck = Checkpoint::load(tmp.path().to_path_buf(), "test", "1.0").unwrap();
        let state_path = tmp.path().join(".wright-checkpoint.json");
        std::fs::write(&state_path, "{}").unwrap();

        assert!(state_path.exists());
        assert!(prepare_layer.exists());

        ck.invalidate_all();

        assert!(!state_path.exists());
        assert!(
            prepare_layer.exists(),
            "layers directory should be preserved"
        );
    }
}

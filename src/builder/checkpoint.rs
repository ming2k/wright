use std::path::PathBuf;

/// Content-addressed stage checkpoints for a single plan build.
///
/// Checkpoints are an internal build-pipeline optimisation.  They live inside
/// the plan's `work_dir` and record which lifecycle stages have already
/// finished for a *specific* plan fingerprint.  If the plan changes, old
/// checkpoints are automatically ignored.
///
/// Workflow state (SQLite) is the source of truth for "should this plan be
/// built at all".  Checkpoints are the source of truth for "which stages
/// inside this plan can be skipped".  They never influence workflow scheduling.
pub struct StageCheckpoint {
    work_dir: PathBuf,
    phase: Option<String>,
}

impl StageCheckpoint {
    pub fn new(work_dir: PathBuf, phase: Option<String>) -> Self {
        Self { work_dir, phase }
    }

    /// Mark a stage as complete, storing the plan fingerprint inside the
    /// sentinel so that future runs can verify the checkpoint is still valid.
    pub fn mark_complete(&self, stage: &str, fingerprint: &str) {
        let path = self.sentinel_path(stage);
        let content = format!("fingerprint={}\n", fingerprint);
        if let Err(e) = std::fs::write(&path, content) {
            tracing::warn!("failed to write stage checkpoint {}: {}", path.display(), e);
        }
    }

    /// Return `true` if the stage has a checkpoint **and** the stored
    /// fingerprint matches `expected_fingerprint`.
    pub fn is_complete(&self, stage: &str, expected_fingerprint: &str) -> bool {
        let path = self.sentinel_path(stage);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let stored = content
                    .lines()
                    .find(|l| l.starts_with("fingerprint="))
                    .and_then(|l| l.strip_prefix("fingerprint="));
                match stored {
                    Some(fp) if fp == expected_fingerprint => true,
                    Some(_) => {
                        tracing::debug!(
                            "checkpoint fingerprint mismatch for stage {} — re-running",
                            stage
                        );
                        false
                    }
                    None => {
                        tracing::debug!(
                            "legacy checkpoint for stage {} (no fingerprint) — re-running",
                            stage
                        );
                        false
                    }
                }
            }
            Err(_) => false,
        }
    }

    /// Remove the checkpoint for a single stage.
    pub fn invalidate(&self, stage: &str) {
        let path = self.sentinel_path(stage);
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }

    /// Remove checkpoints for `stage` and every stage that comes *after* it
    /// in the canonical pipeline order.  Used when a preceding phase (e.g. mvp)
    /// invalidates downstream work.
    pub fn invalidate_from(&self, stage: &str) {
        let order = [
            "fetch",
            "verify",
            "extract",
            "prepare",
            "configure",
            "compile",
            "check",
            "staging",
        ];
        let Some(pos) = order.iter().position(|&s| s == stage) else {
            return;
        };
        for s in &order[pos..] {
            self.invalidate(s);
        }
    }

    /// Remove all stage checkpoints.
    pub fn invalidate_all(&self) {
        let order = [
            "fetch",
            "verify",
            "extract",
            "prepare",
            "configure",
            "compile",
            "check",
            "staging",
        ];
        for s in &order {
            self.invalidate(s);
        }
    }

    fn sentinel_path(&self, stage: &str) -> PathBuf {
        let prefix = match self.phase.as_deref() {
            Some("mvp") => ".wright-stage-mvp",
            _ => ".wright-stage",
        };
        self.work_dir.join(format!("{}-{}", prefix, stage))
    }
}

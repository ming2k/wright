use std::path::PathBuf;
use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};
use crate::foundry::executor::{self, ExecutorOptions};
use crate::foundry::logging;
use wright_plan::manifest::PipelineStage;

use super::Forge;
use super::pipeline::executor_for_stage;

impl<'a> Forge<'a> {
    pub(super) async fn run_ordered_stage_in_target(&self, stage_name: &str) -> Result<()> {
        if stage_name == "configure" {
            let _permit = if let Some(ref s) = self.configure_lock {
                Some(s.acquire().await.expect("configure semaphore closed"))
            } else {
                None
            };
            self.run_stage_with_hooks_in_target(stage_name, self.cpu_count)
                .await
        } else if stage_name == "compile" {
            let effective_cpu = self.compile_cpu_count.unwrap_or(self.cpu_count);
            let _permit = if let Some(ref s) = self.compile_lock {
                Some(
                    s.acquire_many(effective_cpu)
                        .await
                        .expect("compile semaphore closed"),
                )
            } else {
                None
            };
            self.run_stage_with_hooks_in_target(stage_name, effective_cpu)
                .await
        } else {
            self.run_stage_with_hooks_in_target(stage_name, self.cpu_count)
                .await
        }
    }

    async fn run_stage_with_hooks_in_target(&self, stage_name: &str, cpu_count: u32) -> Result<()> {
        let plan_name = &self.manifest.metadata.name;
        let pre_hook = format!("pre_{stage_name}");
        if let Some(stage) = self.get_stage(&pre_hook) {
            debug!(event = "hook.running", plan_name = %plan_name, hook = %pre_hook, "Running pre-hook");
            self.run_stage_in_target(&pre_hook, stage, cpu_count)
                .await?;
        }

        if let Some(stage) = self.get_stage(stage_name) {
            self.run_stage_in_target(stage_name, stage, cpu_count)
                .await?;
            info!(event = "stage.completed", plan_name = %plan_name, stage_name = %stage_name, "Stage completed");
        } else {
            debug!(event = "stage.undefined", plan_name = %plan_name, stage_name = %stage_name, "Skipping undefined stage");
        }

        let post_hook = format!("post_{stage_name}");
        if let Some(stage) = self.get_stage(&post_hook) {
            debug!(event = "hook.running", plan_name = %plan_name, hook = %post_hook, "Running post-hook");
            self.run_stage_in_target(&post_hook, stage, cpu_count)
                .await?;
        }

        Ok(())
    }

    // Legacy single-stage execution for --stage mode (no overlay layering).
    pub(super) async fn run_ordered_stage(&self, stage_name: &str) -> Result<()> {
        if stage_name == "configure" {
            let _permit = if let Some(ref s) = self.configure_lock {
                Some(s.acquire().await.expect("configure semaphore closed"))
            } else {
                None
            };
            self.run_stage_with_hooks(stage_name, self.cpu_count).await
        } else if stage_name == "compile" {
            let effective_cpu = self.compile_cpu_count.unwrap_or(self.cpu_count);
            let _permit = if let Some(ref s) = self.compile_lock {
                Some(
                    s.acquire_many(effective_cpu)
                        .await
                        .expect("compile semaphore closed"),
                )
            } else {
                None
            };
            self.run_stage_with_hooks(stage_name, effective_cpu).await
        } else {
            self.run_stage_with_hooks(stage_name, self.cpu_count).await
        }
    }

    async fn run_stage_with_hooks(&self, stage_name: &str, cpu_count: u32) -> Result<()> {
        let pre_hook = format!("pre_{stage_name}");
        if let Some(stage) = self.get_stage(&pre_hook) {
            debug!("Running hook: {pre_hook}");
            self.run_stage_legacy(&pre_hook, stage, cpu_count).await?;
        }

        if let Some(stage) = self.get_stage(stage_name) {
            self.run_stage_legacy(stage_name, stage, cpu_count).await?;
            info!(
                event = "stage.completed",
                plan_name = %self.manifest.metadata.name,
                stage_name = %stage_name,
                "stage completed"
            );
        } else {
            debug!("Skipping undefined stage: {stage_name}");
        }

        let post_hook = format!("post_{stage_name}");
        if let Some(stage) = self.get_stage(&post_hook) {
            debug!("Running hook: {post_hook}");
            self.run_stage_legacy(&post_hook, stage, cpu_count).await?;
        }

        Ok(())
    }

    async fn run_stage_in_target(
        &self,
        stage_name: &str,
        stage: &PipelineStage,
        cpu_count: u32,
    ) -> Result<()> {
        if stage.script.is_empty() {
            debug!("Stage {stage_name} has empty script, skipping");
            return Ok(());
        }

        let working_dir = self.layers.target_dir();
        let executor = executor_for_stage(stage_name, stage, self.executors)?;
        let isolation_level = executor::resolve_isolation(
            stage_name,
            stage.isolation.as_deref(),
            executor,
            self.default_isolation,
        )?;

        let expanded_script = crate::foundry::variables::substitute(&stage.script, &self.vars);
        let log_path = self.logs_dir.join(format!("{stage_name}.log"));

        let mut stdout_log_file = std::fs::File::create(&log_path).ok().and_then(|mut f| {
            use std::io::Write;
            let ok = write!(
                f,
                "=== Stage: {stage_name} ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n\n--- stdout ---\n",
                working_dir.display(),
                expanded_script.trim()
            ).is_ok();
            if ok { Some(f) } else { None }
        });

        let stdout_log_path_owned = PathBuf::from(&log_path);

        let _stage_span = crate::cli_span!(
            logging::stage_verb(stage_name),
            "{}",
            self.manifest.metadata.name
        );
        debug!(
            event = "stage.started",
            plan_name = %self.manifest.metadata.name,
            stage_name = %stage_name,
            isolation = ?isolation_level,
            "stage started"
        );

        let max_etxtbsy_retries: u32 = 10;
        let mut attempt: u32 = 0;
        let (result, final_attempt) = loop {
            let log_stdout = if attempt > 0 {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&stdout_log_path_owned)
                    .ok()
            } else {
                stdout_log_file.take()
            };

            let mut options = ExecutorOptions {
                level: isolation_level,
                base_root: self.base_root.clone(),
                work_dir: working_dir.to_path_buf(),
                output_dir: self.output_dir.clone(),
                rlimits: self.rlimits.clone(),
                main_part_dir: None,
                verbose: self.verbose,
                cpu_count: Some(cpu_count),
                log_stdout,
                dep_mounts: Vec::new(),
            };

            let res = match executor::execute_script(
                executor,
                &stage.script,
                working_dir,
                &stage.env,
                &self.vars,
                &mut options,
            )
            .await
            {
                Ok(res) => res,
                Err(e) => return Err(e),
            };

            let code = res.status.code().unwrap_or(-1);
            let is_etxtbsy = code == 126
                && (res.stderr.tail.contains("Text file busy")
                    || res.stdout.tail.contains("Text file busy"));

            if is_etxtbsy && attempt < max_etxtbsy_retries {
                attempt += 1;
                let exp_base = 200_u64.saturating_mul(1_u64 << attempt.min(2)).min(1000);
                let delay_ms = exp_base + jitter_ms(exp_base);
                warn!(
                    "[{}] ETXTBUSY in stage '{stage_name}', retrying in {delay_ms}ms (attempt {attempt}/{max_etxtbsy_retries})",
                    self.manifest.metadata.name,
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                continue;
            }
            break (res, attempt);
        };

        let mut result = result;
        if final_attempt > 0 {
            info!(
                "[{}] Stage '{stage_name}' succeeded after {final_attempt} ETXTBUSY retries",
                self.manifest.metadata.name,
            );
        }

        let exit_code = result.status.code().unwrap_or(-1);

        if let Ok(mut log_file) = std::fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = log_file.write_all(b"\n--- stderr ---\n");
            let _ = std::io::copy(&mut result.stderr.file, &mut log_file);
            let _ = write!(log_file, "\n=== Exit code: {exit_code} ===\n",);
        }

        if exit_code != 0 {
            return Err(WrightError::ForgeError(format!(
                "stage '{stage_name}' failed with exit code {exit_code} (see log: {})",
                log_path.display()
            )));
        }

        Ok(())
    }

    async fn run_stage_legacy(
        &self,
        stage_name: &str,
        stage: &PipelineStage,
        cpu_count: u32,
    ) -> Result<()> {
        if stage.script.is_empty() {
            debug!("Stage {stage_name} has empty script, skipping");
            return Ok(());
        }

        let executor = executor_for_stage(stage_name, stage, self.executors)?;
        let isolation_level = executor::resolve_isolation(
            stage_name,
            stage.isolation.as_deref(),
            executor,
            self.default_isolation,
        )?;

        let expanded_script = crate::foundry::variables::substitute(&stage.script, &self.vars);
        let log_path = self.logs_dir.join(format!("{stage_name}.log"));

        let mut stdout_log_file = std::fs::File::create(&log_path).ok().and_then(|mut f| {
            use std::io::Write;
            let ok = write!(
                f,
                "=== Stage: {stage_name} ===\n=== Working dir: {} ===\n\n--- script ---\n{}\n\n--- stdout ---\n",
                self.layers.target_dir().display(),
                expanded_script.trim()
            ).is_ok();
            if ok { Some(f) } else { None }
        });

        let stdout_log_path_owned = PathBuf::from(&log_path);

        let _stage_span = crate::cli_span!(
            logging::stage_verb(stage_name),
            "{}",
            self.manifest.metadata.name
        );
        debug!(
            event = "stage.started",
            plan_name = %self.manifest.metadata.name,
            stage_name = %stage_name,
            isolation = ?isolation_level,
            "stage started"
        );

        let max_etxtbsy_retries: u32 = 10;
        let mut attempt: u32 = 0;
        let (result, final_attempt) = loop {
            let log_stdout = if attempt > 0 {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&stdout_log_path_owned)
                    .ok()
            } else {
                stdout_log_file.take()
            };

            let mut options = ExecutorOptions {
                level: isolation_level,
                base_root: self.base_root.clone(),
                work_dir: self.layers.target_dir().to_path_buf(),
                output_dir: self.output_dir.clone(),
                rlimits: self.rlimits.clone(),
                main_part_dir: None,
                verbose: self.verbose,
                cpu_count: Some(cpu_count),
                log_stdout,
                dep_mounts: Vec::new(),
            };

            let res = executor::execute_script(
                executor,
                &stage.script,
                self.layers.target_dir(),
                &stage.env,
                &self.vars,
                &mut options,
            )
            .await?;

            let code = res.status.code().unwrap_or(-1);
            let is_etxtbsy = code == 126
                && (res.stderr.tail.contains("Text file busy")
                    || res.stdout.tail.contains("Text file busy"));

            if is_etxtbsy && attempt < max_etxtbsy_retries {
                attempt += 1;
                let exp_base = 200_u64.saturating_mul(1_u64 << attempt.min(2)).min(1000);
                let delay_ms = exp_base + jitter_ms(exp_base);
                warn!(
                    "[{}] ETXTBUSY in stage '{stage_name}', retrying in {delay_ms}ms (attempt {attempt}/{max_etxtbsy_retries})",
                    self.manifest.metadata.name,
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                continue;
            }
            break (res, attempt);
        };

        let mut result = result;
        if final_attempt > 0 {
            info!(
                "[{}] Stage '{stage_name}' succeeded after {final_attempt} ETXTBUSY retries",
                self.manifest.metadata.name,
            );
        }

        let exit_code = result.status.code().unwrap_or(-1);

        if let Ok(mut log_file) = std::fs::OpenOptions::new().append(true).open(&log_path) {
            use std::io::Write;
            let _ = log_file.write_all(b"\n--- stderr ---\n");
            let _ = std::io::copy(&mut result.stderr.file, &mut log_file);
            let _ = write!(log_file, "\n=== Exit code: {exit_code} ===\n",);
        }

        if exit_code != 0 {
            return Err(WrightError::ForgeError(format!(
                "stage '{stage_name}' failed with exit code {exit_code} (see log: {})",
                log_path.display()
            )));
        }

        Ok(())
    }
}

fn jitter_ms(max_ms: u64) -> u64 {
    if max_ms == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    nanos.wrapping_mul(2654435761).wrapping_add(pid) % max_ms
}

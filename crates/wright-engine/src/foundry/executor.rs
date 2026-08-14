use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::isolation::{
    IsolationConfig, IsolationLevel, IsolationOutput, ResourceLimits, run_in_isolation,
};

#[derive(Debug, Deserialize, Clone)]
pub struct ExecutorConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_delivery")]
    pub delivery: String,
    #[serde(default = "default_extension")]
    pub tempfile_extension: String,
    #[serde(default)]
    pub required_paths: Vec<String>,
    #[serde(default)]
    pub default_isolation: String,
}

fn default_delivery() -> String {
    "tempfile".to_string()
}

fn default_extension() -> String {
    ".sh".to_string()
}

fn add_required_paths(executor: &ExecutorConfig, config: &mut IsolationConfig) -> Result<()> {
    if config.level == IsolationLevel::None {
        return Ok(());
    }

    for required in &executor.required_paths {
        let path = PathBuf::from(required);
        if !path.is_absolute() {
            return Err(WrightError::ForgeError(format!(
                "executor {} required path must be absolute: {}",
                executor.name, required
            )));
        }
        if !path.exists() {
            return Err(WrightError::ForgeError(format!(
                "executor {} required path does not exist: {}",
                executor.name, required
            )));
        }
        config.extra_binds.push((path.clone(), path, true));
    }
    Ok(())
}

/// Resolve a stage's effective policy in one place so execution, checkpoint
/// invalidation, and provenance use exactly the same precedence:
/// stage override > executor default > global build default.
pub(crate) fn resolve_isolation(
    stage_name: &str,
    stage_override: Option<&str>,
    executor: &ExecutorConfig,
    global_default: IsolationLevel,
) -> Result<IsolationLevel> {
    let Some(raw) = stage_override.or_else(|| {
        (!executor.default_isolation.is_empty()).then_some(executor.default_isolation.as_str())
    }) else {
        return Ok(global_default);
    };

    raw.parse::<IsolationLevel>().map_err(|error| {
        WrightError::context(
            format!(
                "stage '{stage_name}' has invalid effective isolation '{raw}' for executor '{}'",
                executor.name
            ),
            error,
        )
    })
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            name: "shell".to_string(),
            description: "Bash shell executor".to_string(),
            command: "/bin/bash".to_string(),
            args: vec!["-e".to_string(), "-o".to_string(), "pipefail".to_string()],
            delivery: "tempfile".to_string(),
            tempfile_extension: ".sh".to_string(),
            required_paths: vec![],
            // The built-in shell follows the global build default unless a
            // pipeline stage explicitly overrides it.
            default_isolation: String::new(),
        }
    }
}

pub struct ExecutorRegistry {
    executors: HashMap<String, ExecutorConfig>,
}

impl Default for ExecutorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutorRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            executors: HashMap::new(),
        };
        registry
            .executors
            .insert("shell".to_string(), ExecutorConfig::default());
        registry
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir).map_err(WrightError::IoError)? {
            let entry = entry.map_err(WrightError::IoError)?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                let content = std::fs::read_to_string(&path).map_err(WrightError::IoError)?;
                let config: ExecutorWrapper = toml::from_str(&content)?;
                if !config.executor.default_isolation.is_empty() {
                    config
                        .executor
                        .default_isolation
                        .parse::<IsolationLevel>()
                        .map_err(|error| {
                            WrightError::context(
                                format!(
                                    "executor '{}' in {} has invalid default_isolation '{}'",
                                    config.executor.name,
                                    path.display(),
                                    config.executor.default_isolation
                                ),
                                error,
                            )
                        })?;
                }
                debug!(
                    "Loaded executor: {} from {}",
                    config.executor.name,
                    path.display()
                );
                self.executors
                    .insert(config.executor.name.clone(), config.executor);
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ExecutorConfig> {
        self.executors.get(name)
    }
}

#[derive(Deserialize)]
struct ExecutorWrapper {
    executor: ExecutorConfig,
}

#[derive(Debug)]
pub struct ExecutorOptions {
    pub level: IsolationLevel,
    pub base_root: PathBuf,
    pub work_dir: PathBuf,
    pub output_dir: PathBuf,
    pub rlimits: ResourceLimits,
    pub main_part_dir: Option<PathBuf>,
    pub verbose: bool,
    pub cpu_count: Option<u32>,
    pub log_stdout: Option<std::fs::File>,
    pub dep_mounts: Vec<(PathBuf, PathBuf)>,
}

pub async fn execute_script(
    executor: &ExecutorConfig,
    script: &str,
    working_dir: &Path,
    env_vars: &HashMap<String, String>,
    vars: &HashMap<String, String>,
    options: &mut ExecutorOptions,
) -> Result<IsolationOutput> {
    let effective_vars = if options.level != IsolationLevel::None {
        let mut v = vars.clone();
        v.insert("WORKDIR".to_string(), "/build".to_string());
        v.insert("STAGING_DIR".to_string(), "/output".to_string());
        v.insert(
            "MAIN_STAGING_DIR".to_string(),
            if options.main_part_dir.is_some() {
                "/main-part".to_string()
            } else {
                "/output".to_string()
            },
        );
        v
    } else {
        vars.clone()
    };

    let expanded = variables::substitute(script, &effective_vars);
    let script_name = format!(".wright_script{}", executor.tempfile_extension);
    // The stage's working tree is a real directory populated from the
    // merged base, so the script simply lands in the working directory.
    let script_dir = working_dir.to_path_buf();
    let script_path = script_dir.join(&script_name);
    tokio::fs::write(&script_path, &expanded)
        .await
        .map_err(|e| WrightError::context("failed to write forge script", e))?;

    // Defensive sync: on some kernels/fs configs, a file written via async I/O
    // may briefly appear busy to execve.  Ensure the script is fully persisted
    // before we hand it to the executor.
    if let Ok(file) = tokio::fs::File::open(&script_path).await {
        let _ = file.sync_all().await;
    }

    let task_id = format!(
        "{}-{}",
        vars.get("NAME")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut config = IsolationConfig::new(
        options.level,
        options.work_dir.clone(),
        options.output_dir.clone(),
        task_id,
    );

    config.base_root = options.base_root.clone();
    config.rlimits = options.rlimits.clone();
    config.verbose = options.verbose;
    config.cpu_count = options.cpu_count;
    config.log_stdout = options.log_stdout.take();
    config.dep_mounts = std::mem::take(&mut options.dep_mounts);

    if let Some(ref main_part) = options.main_part_dir {
        config
            .extra_binds
            .push((main_part.clone(), PathBuf::from("/main-part"), false));
    }

    add_required_paths(executor, &mut config)?;

    for (key, value) in env_vars {
        let expanded_value = variables::substitute(value, &effective_vars);
        config.env.push((key.clone(), expanded_value));
    }

    for (key, value) in &effective_vars {
        if !config.env.iter().any(|(k, _)| k == key) {
            config.env.push((key.clone(), value.clone()));
        }
    }

    for key in [
        "CC",
        "CXX",
        "AR",
        "AS",
        "LD",
        "NM",
        "RANLIB",
        "STRIP",
        "OBJCOPY",
        "OBJDUMP",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "C_INCLUDE_PATH",
        "CPLUS_INCLUDE_PATH",
        "LIBRARY_PATH",
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_SYSROOT_DIR",
        "MAKEFLAGS",
        "JOBS",
    ] {
        if let Ok(value) = std::env::var(key)
            && !config.env.iter().any(|(k, _)| k == key)
        {
            config.env.push((key.to_string(), value));
        }
    }

    let mut args = executor.args.clone();
    if executor.delivery == "tempfile" {
        if options.level == IsolationLevel::None {
            args.push(script_path.to_string_lossy().to_string());
        } else {
            args.push(format!("/build/{}", script_name));
        }
    }

    let command = executor.command.clone();
    let mut output =
        tokio::task::spawn_blocking(move || run_in_isolation(&mut config, &command, &args))
            .await
            .map_err(|e| WrightError::context("spawn_blocking failed", e))??;

    if output.status.code() != Some(0) {
        let mut remapped_stderr = output.stderr.tail.clone();
        remapped_stderr = remapped_stderr.replace("/main-part", "${MAIN_STAGING_DIR}");
        remapped_stderr = remapped_stderr.replace("/output", "${STAGING_DIR}");
        remapped_stderr = remapped_stderr.replace("/build", "${WORKDIR}");
        output.stderr.tail = remapped_stderr;
    }

    Ok(IsolationOutput {
        stdout: output.stdout,
        stderr: output.stderr,
        status: output.status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolation_config(level: IsolationLevel) -> (tempfile::TempDir, IsolationConfig) {
        let root = tempfile::tempdir().unwrap();
        let config = IsolationConfig::new(
            level,
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            "executor-test".to_string(),
        );
        (root, config)
    }

    #[test]
    fn required_paths_become_read_only_binds() {
        let required = tempfile::tempdir().unwrap();
        let executor = ExecutorConfig {
            required_paths: vec![required.path().display().to_string()],
            ..ExecutorConfig::default()
        };
        let (_root, mut config) = isolation_config(IsolationLevel::Strict);

        add_required_paths(&executor, &mut config).unwrap();

        assert_eq!(
            config.extra_binds,
            vec![(
                required.path().to_path_buf(),
                required.path().to_path_buf(),
                true
            )]
        );
    }

    #[test]
    fn required_paths_are_ignored_for_explicit_host_execution() {
        let executor = ExecutorConfig {
            required_paths: vec!["relative-and-missing".to_string()],
            ..ExecutorConfig::default()
        };
        let (_root, mut config) = isolation_config(IsolationLevel::None);

        add_required_paths(&executor, &mut config).unwrap();

        assert!(config.extra_binds.is_empty());
    }

    #[test]
    fn isolated_required_paths_must_be_absolute() {
        let executor = ExecutorConfig {
            required_paths: vec!["relative".to_string()],
            ..ExecutorConfig::default()
        };
        let (_root, mut config) = isolation_config(IsolationLevel::Strict);

        let error = add_required_paths(&executor, &mut config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must be absolute"));
    }

    #[test]
    fn isolation_precedence_is_stage_then_executor_then_global() {
        let executor = ExecutorConfig {
            default_isolation: "relaxed".to_string(),
            ..ExecutorConfig::default()
        };

        assert_eq!(
            resolve_isolation("compile", Some("none"), &executor, IsolationLevel::Strict).unwrap(),
            IsolationLevel::None
        );
        assert_eq!(
            resolve_isolation("compile", None, &executor, IsolationLevel::Strict).unwrap(),
            IsolationLevel::Relaxed
        );

        let shell = ExecutorConfig::default();
        assert_eq!(
            resolve_isolation("compile", None, &shell, IsolationLevel::None).unwrap(),
            IsolationLevel::None
        );
    }

    #[test]
    fn invalid_executor_isolation_is_rejected_when_inherited() {
        let executor = ExecutorConfig {
            default_isolation: "container".to_string(),
            ..ExecutorConfig::default()
        };

        let error = resolve_isolation("compile", None, &executor, IsolationLevel::Strict)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid effective isolation"));
    }
}

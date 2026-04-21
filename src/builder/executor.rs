use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::builder::variables;
use crate::error::{Result, WrightError};
use crate::isolation::{
    run_in_isolation, IsolationConfig, IsolationLevel, IsolationOutput, ResourceLimits,
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
            default_isolation: "strict".to_string(),
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
        v.insert("PART_DIR".to_string(), "/output".to_string());
        v.insert(
            "MAIN_PART_DIR".to_string(),
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
    let script_path = working_dir.join(&script_name);
    tokio::fs::write(&script_path, &expanded)
        .await
        .map_err(|e| WrightError::BuildError(format!("failed to write build script: {}", e)))?;

    let task_id = format!(
        "{}-{}",
        vars.get("NAME")
            .cloned()
            .unwrap_or_else(|| "unknown".to_string()),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
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

    if let Some(ref main_part) = options.main_part_dir {
        config
            .extra_binds
            .push((main_part.clone(), PathBuf::from("/main-part"), false));
    }

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
        if let Ok(value) = std::env::var(key) {
            if !config.env.iter().any(|(k, _)| k == key) {
                config.env.push((key.to_string(), value));
            }
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
            .map_err(|e| WrightError::BuildError(format!("spawn_blocking failed: {}", e)))??;

    if output.status.code() != Some(0) {
        let mut remapped_stderr = output.stderr.tail.clone();
        remapped_stderr = remapped_stderr.replace("/main-part", "${MAIN_PART_DIR}");
        remapped_stderr = remapped_stderr.replace("/output", "${PART_DIR}");
        remapped_stderr = remapped_stderr.replace("/build", "${WORKDIR}");
        output.stderr.tail = remapped_stderr;
    }

    Ok(IsolationOutput {
        stdout: output.stdout,
        stderr: output.stderr,
        status: output.status,
    })
}

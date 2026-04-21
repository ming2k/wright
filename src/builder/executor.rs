use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::debug;

use crate::builder::variables;
use crate::isolation::{run_in_isolation, IsolationConfig, IsolationLevel, ResourceLimits};
use crate::error::{Result, WrightError};

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
        // Register default shell executor
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
    /// Main part's output_dir, mounted at /main-part for split part stages.
    pub main_part_dir: Option<PathBuf>,
    pub verbose: bool,
    /// Number of CPUs to pin the sandboxed process to via sched_setaffinity.
    /// `nproc` inside the sandbox then returns this count naturally.
    pub cpu_count: Option<u32>,
    /// When set, subprocess stdout is tee'd to this file in real time.
    pub log_stdout: Option<std::fs::File>,
}

pub struct ExecutionResult {
    pub stdout: crate::isolation::CapturedOutput,
    pub stderr: crate::isolation::CapturedOutput,
    pub exit_code: i32,
}

/// Execute a script using a specific executor with sandbox support.
pub fn execute_script(
    executor: &ExecutorConfig,
    script: &str,
    working_dir: &Path,
    env_vars: &HashMap<String, String>,
    vars: &HashMap<String, String>,
    options: &mut ExecutorOptions,
) -> Result<ExecutionResult> {
    // When running in a sandbox, remap path variables to sandbox mount points
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

    // Write script to a hidden file in working_dir to keep it clean but accessible
    let script_name = format!(".wright_script{}", executor.tempfile_extension);
    let script_path = working_dir.join(&script_name);
    std::fs::write(&script_path, &expanded)
        .map_err(|e| WrightError::BuildError(format!("failed to write build script: {}", e)))?;

    // Create sandbox config
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

    // Mount main part dir for split part stages
    if let Some(ref main_part) = options.main_part_dir {
        config
            .extra_binds
            .push((main_part.clone(), PathBuf::from("/main-part"), false));
    }

    // Set environment variables
    for (key, value) in env_vars {
        let expanded_value = variables::substitute(value, &effective_vars);
        config.env.push((key.clone(), expanded_value));
    }

    // Expose build variables (use sandbox paths when sandboxed).
    // Don't override variables already set by the stage env above.
    for (key, value) in &effective_vars {
        if !config.env.iter().any(|(k, _)| k == key) {
            config.env.push((key.clone(), value.clone()));
        }
    }

    // Pass through standard build environment variables from the host.
    // This is important for bootstrap/stage1 environments where paths
    // like C_INCLUDE_PATH or LIBRARY_PATH are set to non-standard locations.
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
            // Don't override if already set by the plan manifest.
            if !config.env.iter().any(|(k, _)| k == key) {
                config.env.push((key.to_string(), value));
            }
        }
    }

    // Build arguments for the command
    let mut args = executor.args.clone();
    if executor.delivery == "tempfile" {
        if options.level == IsolationLevel::None {
            // Running directly on the host: use the real path
            args.push(script_path.to_string_lossy().to_string());
        } else {
            // In sandbox, working_dir is mounted at /build
            args.push(format!("/build/{}", script_name));
        }
    }

    // Execute in isolation
    let mut output = run_in_isolation(&mut config, &executor.command, &args)?;

    // Golden Standard: Map internal sandbox paths back to variables in stderr
    // so the user sees "${PART_DIR}/..." instead of "/output/...".
    if output.status.code() != Some(0) {
        let mut remapped_stderr = output.stderr.tail.clone();
        remapped_stderr = remapped_stderr.replace("/main-part", "${MAIN_PART_DIR}");
        remapped_stderr = remapped_stderr.replace("/output", "${PART_DIR}");
        remapped_stderr = remapped_stderr.replace("/build", "${WORKDIR}");
        output.stderr.tail = remapped_stderr;
    }

    let exit_code = output.status.code().unwrap_or(-1);

    Ok(ExecutionResult {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code,
    })
}

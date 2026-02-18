use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::info;

use crate::error::{WrightError, Result};
use crate::builder::variables;
use crate::sandbox::{self, ResourceLimits, SandboxConfig, SandboxLevel};

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
    pub default_sandbox: String,
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
            default_sandbox: "strict".to_string(),
        }
    }
}

pub struct ExecutorRegistry {
    executors: HashMap<String, ExecutorConfig>,
}

impl ExecutorRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            executors: HashMap::new(),
        };
        // Register default shell executor
        registry.executors.insert("shell".to_string(), ExecutorConfig::default());
        registry
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir).map_err(|e| WrightError::IoError(e))? {
            let entry = entry.map_err(|e| WrightError::IoError(e))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                let content = std::fs::read_to_string(&path).map_err(|e| WrightError::IoError(e))?;
                let config: ExecutorWrapper = toml::from_str(&content)?;
                info!("Loaded executor: {} from {}", config.executor.name, path.display());
                self.executors.insert(config.executor.name.clone(), config.executor);
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

#[derive(Debug, Clone)]
pub struct ExecutorOptions {
    pub level: SandboxLevel,
    pub src_dir: PathBuf,
    pub pkg_dir: PathBuf,
    pub files_dir: Option<PathBuf>,
    pub rlimits: ResourceLimits,
    /// Main package's pkg_dir, mounted at /main-pkg for split package stages.
    pub main_pkg_dir: Option<PathBuf>,
}

pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Execute a script using a specific executor with sandbox support.
pub fn execute_script(
    executor: &ExecutorConfig,
    script: &str,
    working_dir: &Path,
    env_vars: &HashMap<String, String>,
    vars: &HashMap<String, String>,
    options: &ExecutorOptions,
) -> Result<ExecutionResult> {
    // When running in a sandbox, remap path variables to sandbox mount points
    let effective_vars = if options.level != SandboxLevel::None {
        let mut v = vars.clone();
        // Remap BUILD_DIR: replace the host SRC_DIR prefix with /build
        if let (Some(host_build_dir), Some(host_src_dir)) = (vars.get("BUILD_DIR"), vars.get("SRC_DIR")) {
            if let Some(suffix) = host_build_dir.strip_prefix(host_src_dir.as_str()) {
                v.insert("BUILD_DIR".to_string(), format!("/build{}", suffix));
            } else {
                v.insert("BUILD_DIR".to_string(), "/build".to_string());
            }
        }
        v.insert("SRC_DIR".to_string(), "/build".to_string());
        v.insert("PKG_DIR".to_string(), "/output".to_string());
        if options.files_dir.is_some() {
            v.insert("FILES_DIR".to_string(), "/files".to_string());
        }
        if options.main_pkg_dir.is_some() {
            v.insert("MAIN_PKG_DIR".to_string(), "/main-pkg".to_string());
        }
        v
    } else {
        vars.clone()
    };

    let expanded = variables::substitute(script, &effective_vars);

    // Write script to a hidden file in working_dir to keep it clean but accessible
    let script_name = format!(".wright_script{}", executor.tempfile_extension);
    let script_path = working_dir.join(&script_name);
    std::fs::write(&script_path, &expanded).map_err(|e| {
        WrightError::BuildError(format!("failed to write build script: {}", e))
    })?;

    // Create sandbox config
    let mut config = SandboxConfig::new(options.level, options.src_dir.clone(), options.pkg_dir.clone());
    config.files_dir = options.files_dir.clone();
    config.rlimits = options.rlimits.clone();

    // Mount main package dir for split package stages
    if let Some(ref main_pkg) = options.main_pkg_dir {
        config.extra_binds.push((main_pkg.clone(), PathBuf::from("/main-pkg"), false));
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

    // Auto-inject parallel job limits so build tools respect `jobs` without
    // the user having to manually pass `-j${NPROC}` in every script.
    if let Some(nproc) = effective_vars.get("NPROC") {
        let nproc_val = nproc.clone();
        // cmake --build (controls Ninja/Make spawned by cmake)
        if !config.env.iter().any(|(k, _)| k == "CMAKE_BUILD_PARALLEL_LEVEL") {
            config.env.push(("CMAKE_BUILD_PARALLEL_LEVEL".to_string(), nproc_val.clone()));
        }
        // make
        if !config.env.iter().any(|(k, _)| k == "MAKEFLAGS") {
            config.env.push(("MAKEFLAGS".to_string(), format!("-j{}", nproc_val)));
        }
    }

    // Pass through standard build environment variables from the host.
    // This is important for bootstrap/stage1 environments where paths
    // like C_INCLUDE_PATH or LIBRARY_PATH are set to non-standard locations.
    for key in [
        "CC", "CXX", "AR", "AS", "LD", "NM", "RANLIB", "STRIP", "OBJCOPY", "OBJDUMP",
        "CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS",
        "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "LIBRARY_PATH",
        "PKG_CONFIG_PATH", "PKG_CONFIG_SYSROOT_DIR",
        "MAKEFLAGS", "JOBS",
    ] {
        if let Ok(value) = std::env::var(key) {
            // Don't override if already set by the package manifest.
            if !config.env.iter().any(|(k, _)| k == key) {
                config.env.push((key.to_string(), value));
            }
        }
    }

    // Build arguments for the command
    let mut args = executor.args.clone();
    if executor.delivery == "tempfile" {
        if options.level == SandboxLevel::None {
            // Running directly on the host: use the real path
            args.push(script_path.to_string_lossy().to_string());
        } else {
            // In sandbox, working_dir is mounted at /build
            args.push(format!("/build/{}", script_name));
        }
    }

    // Execute in sandbox
    let output = sandbox::run_in_sandbox(&config, &executor.command, &args)?;

    let exit_code = output.status.code().unwrap_or(-1);

    Ok(ExecutionResult {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code,
    })
}

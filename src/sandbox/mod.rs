pub mod bwrap;

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLevel {
    None,
    Relaxed,
    Strict,
}

impl SandboxLevel {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "none" => Self::None,
            "relaxed" => Self::Relaxed,
            "strict" | _ => Self::Strict,
        }
    }
}

pub struct SandboxConfig {
    pub level: SandboxLevel,
    pub src_dir: PathBuf,
    pub pkg_dir: PathBuf,
    pub patches_dir: Option<PathBuf>,
    pub extra_binds: Vec<(PathBuf, PathBuf, bool)>, // (host_path, dest_path, read_only)
    pub env: Vec<(String, String)>,
}

impl SandboxConfig {
    pub fn new(level: SandboxLevel, src_dir: PathBuf, pkg_dir: PathBuf) -> Self {
        Self {
            level,
            src_dir,
            pkg_dir,
            patches_dir: None,
            extra_binds: Vec::new(),
            env: Vec::new(),
        }
    }
}

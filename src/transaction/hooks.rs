use std::path::Path;

use crate::error::{Result, WrightError};

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct HooksFile {
    #[serde(default)]
    hooks: Hooks,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(super) struct Hooks {
    #[serde(default)]
    pub pre_install: Option<String>,
    #[serde(default)]
    pub post_install: Option<String>,
    #[serde(default)]
    pub post_upgrade: Option<String>,
    #[serde(default)]
    pub pre_remove: Option<String>,
    #[serde(default)]
    pub post_remove: Option<String>,
}

pub(super) fn read_hooks(extract_dir: &Path) -> (Option<String>, Hooks) {
    let hooks_path = extract_dir.join(".HOOKS");
    if let Ok(content) = std::fs::read_to_string(&hooks_path) {
        if let Ok(parsed) = toml::from_str::<HooksFile>(&content) {
            return (Some(content), parsed.hooks);
        }
    }

    (None, Hooks::default())
}

pub(super) fn parse_hooks_from_db(content: &str) -> Hooks {
    if let Ok(parsed) = toml::from_str::<HooksFile>(content) {
        return parsed.hooks;
    }
    Hooks::default()
}

pub fn get_hook(content: &str, hook_name: &str) -> Option<String> {
    let hooks = parse_hooks_from_db(content);
    match hook_name {
        "pre_install" => hooks.pre_install,
        "post_install" => hooks.post_install,
        "post_upgrade" => hooks.post_upgrade,
        "pre_remove" => hooks.pre_remove,
        "post_remove" => hooks.post_remove,
        _ => None,
    }
}

pub(super) fn run_install_script(script: &str, root_dir: &Path) -> Result<()> {
    let use_chroot = root_dir != Path::new("/") && nix::unistd::geteuid().is_root();
    let mut command = if use_chroot {
        let mut command = std::process::Command::new("/usr/bin/chroot");
        command.arg(root_dir).arg("/bin/sh");
        command
    } else {
        std::process::Command::new("/bin/sh")
    };

    let root_env = if use_chroot { Path::new("/") } else { root_dir };
    let current_dir = if use_chroot { Path::new("/") } else { root_dir };

    let status = command
        .arg("-e")
        .arg("-c")
        .arg(script)
        .env("ROOT", root_env)
        .current_dir(current_dir)
        .status()
        .map_err(|e| WrightError::ScriptError(format!("failed to execute script: {}", e)))?;

    if !status.success() {
        return Err(WrightError::ScriptError(format!(
            "script exited with status {}",
            status
        )));
    }
    Ok(())
}

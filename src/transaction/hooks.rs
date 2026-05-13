use std::path::Path;
use std::process::Stdio;

use crate::error::{Result, WrightError};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

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

fn parse_hooks_from_db(content: &str) -> Hooks {
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

pub(super) fn log_running_hook(part_name: &str, hook_name: &str) {
    tracing::info!("Running hook [{}] for {}", hook_name, part_name);
}

fn normalize_hook_output_line(line: &str) -> &str {
    line.strip_prefix(":: ").unwrap_or(line)
}

async fn log_hook_output<R>(
    reader: R,
    part_name: String,
    hook_name: String,
    stderr: bool,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let line = normalize_hook_output_line(line.trim_end());
        if line.is_empty() {
            continue;
        }
        if stderr {
            tracing::warn!("hook [{}] for {}: {}", hook_name, part_name, line);
        } else {
            tracing::info!("hook [{}] for {}: {}", hook_name, part_name, line);
        }
    }
    Ok(())
}

async fn join_hook_output(
    handle: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
) -> Result<()> {
    if let Some(handle) = handle {
        handle
            .await
            .map_err(|e| WrightError::ScriptError(format!("hook output task failed: {}", e)))?
            .map_err(|e| WrightError::ScriptError(format!("failed to read hook output: {}", e)))?;
    }
    Ok(())
}

pub(super) async fn run_deploy_script(
    script: &str,
    root_dir: &Path,
    part_name: &str,
    hook_name: &str,
) -> Result<()> {
    let use_chroot = root_dir != Path::new("/") && nix::unistd::geteuid().is_root();
    let mut command = if use_chroot {
        let mut command = tokio::process::Command::new("/usr/bin/chroot");
        command.arg(root_dir).arg("/bin/sh");
        command
    } else {
        tokio::process::Command::new("/bin/sh")
    };

    let root_env = if use_chroot { Path::new("/") } else { root_dir };
    let current_dir = if use_chroot { Path::new("/") } else { root_dir };

    let mut child = command
        .arg("-e")
        .arg("-c")
        .arg(script)
        .env("ROOT", root_env)
        .current_dir(current_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| WrightError::ScriptError(format!("failed to execute script: {}", e)))?;

    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(log_hook_output(
            stdout,
            part_name.to_string(),
            hook_name.to_string(),
            false,
        ))
    });
    let stderr_task = child.stderr.take().map(|stderr| {
        tokio::spawn(log_hook_output(
            stderr,
            part_name.to_string(),
            hook_name.to_string(),
            true,
        ))
    });

    let status = child
        .wait()
        .await
        .map_err(|e| WrightError::ScriptError(format!("failed to wait for script: {}", e)))?;

    join_hook_output(stdout_task).await?;
    join_hook_output(stderr_task).await?;

    if !status.success() {
        return Err(WrightError::ScriptError(format!(
            "script exited with status {}",
            status
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalize_hook_output_line;

    #[test]
    fn hook_output_drops_pacman_style_prefix() {
        assert_eq!(
            normalize_hook_output_line(":: Create /etc/xray/config.json"),
            "Create /etc/xray/config.json"
        );
        assert_eq!(normalize_hook_output_line("plain output"), "plain output");
    }
}

use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::Path;

use crate::isolation::IsolationConfig;
use crate::isolation::error::{IsolationError, Result};

pub(super) fn prepare_exec(
    config: &IsolationConfig,
    command: &str,
    args: &[String],
) -> Result<(CString, Vec<CString>, Vec<CString>)> {
    if !Path::new(command).is_absolute() {
        return Err(IsolationError::InvalidConfig(format!(
            "isolated command must be an absolute path: {command:?}"
        )));
    }

    let c_command = CString::new(command).map_err(|error| {
        IsolationError::InvalidConfig(format!("command contains a NUL byte: {error}"))
    })?;
    let mut c_args = Vec::with_capacity(args.len() + 1);
    c_args.push(c_command.clone());
    for arg in args {
        c_args.push(CString::new(arg.as_str()).map_err(|error| {
            IsolationError::InvalidConfig(format!("argument contains a NUL byte: {error}"))
        })?);
    }

    let mut environment = BTreeMap::from([
        ("HOME".to_string(), "/build".to_string()),
        (
            "PATH".to_string(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
        ),
        ("TERM".to_string(), "xterm".to_string()),
    ]);
    for (key, value) in &config.env {
        environment.insert(key.clone(), value.clone());
    }
    let c_env = environment
        .into_iter()
        .map(|(key, value)| {
            CString::new(format!("{key}={value}")).map_err(|error| {
                IsolationError::InvalidConfig(format!(
                    "environment variable {key:?} contains a NUL byte: {error}"
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok((c_command, c_args, c_env))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::isolation::IsolationLevel;

    fn config(src: &Path, output: &Path) -> IsolationConfig {
        IsolationConfig::new(
            IsolationLevel::Strict,
            src.to_path_buf(),
            output.to_path_buf(),
            "native-test".to_string(),
        )
    }

    #[test]
    fn isolated_exec_requires_absolute_command() {
        let src = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let config = config(src.path(), output.path());

        let error = prepare_exec(&config, "bash", &[]).unwrap_err().to_string();
        assert!(error.contains("absolute path"));
    }
}

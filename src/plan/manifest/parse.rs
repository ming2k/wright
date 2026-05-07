use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, WrightError};

use super::BuildOptions;
use super::{
    BackupConfig, DiscardRule, FabricateHooks, InstallScripts, LifecycleOrder, LifecycleStage,
    OutputConfig, PhaseConfig, PlanManifest, PlanMetadata, Relations, Source, Sources,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawManifest {
    #[serde(flatten)]
    pub plan: PlanMetadata,
    #[serde(default)]
    pub build_deps: Vec<String>,
    #[serde(default)]
    pub link_deps: Vec<String>,
    #[serde(default)]
    pub sources: Option<toml::Value>,
    #[serde(default)]
    pub options: BuildOptions,
    #[serde(default)]
    pub lifecycle: Option<HashMap<String, toml::Value>>,
    #[serde(default)]
    pub lifecycle_order: Option<LifecycleOrder>,
    /// Top-level [hooks] — legacy syntax; use [[output]].hooks instead.
    #[serde(default)]
    pub hooks: Option<FabricateHooks>,
    #[serde(default)]
    pub output: Option<toml::Value>,
    #[serde(default)]
    pub discard: Vec<DiscardRule>,
}

struct OutputSection {
    outputs: Option<OutputConfig>,
    install_scripts: Option<InstallScripts>,
    backup: Option<BackupConfig>,
    relations: Relations,
    runtime_deps: Vec<String>,
}

fn parse_output_section(
    default_output_name: &str,
    output_val: Option<toml::Value>,
    main_hooks: Option<FabricateHooks>,
) -> Result<OutputSection> {
    match output_val {
        // --- Multi-output mode: [[output]] array-of-tables ---
        // Declaration order is preserved by TOML arrays.
        Some(toml::Value::Array(arr)) => {
            if main_hooks.is_some() {
                return Err(WrightError::ParseError(
                    "top-level [hooks] cannot be used with multi-output plans; \
                     use [[output]] hooks fields"
                        .to_string(),
                ));
            }
            let mut parts: Vec<(String, super::SubFabricateOutput)> = Vec::new();
            for (i, entry) in arr.into_iter().enumerate() {
                let mut table = match entry {
                    toml::Value::Table(t) => t,
                    _ => {
                        return Err(WrightError::ParseError(format!(
                            "[[output]] entry {} must be a table",
                            i
                        )))
                    }
                };
                let name = match table.remove("name") {
                    Some(toml::Value::String(s)) if s.trim().is_empty() => {
                        default_output_name.to_string()
                    }
                    Some(toml::Value::String(s)) => s,
                    Some(_) => {
                        return Err(WrightError::ParseError(format!(
                            "[[output]] entry {}: 'name' must be a string",
                            i
                        )))
                    }
                    None => default_output_name.to_string(),
                };
                let sub: super::SubFabricateOutput =
                    toml::Value::Table(table)
                        .try_into()
                        .map_err(|e: toml::de::Error| {
                            WrightError::ParseError(format!(
                                "failed to parse [[output]] entry '{}': {}",
                                name, e
                            ))
                        })?;
                if matches!(&sub.include, Some(v) if v.is_empty()) {
                    return Err(WrightError::ParseError(format!(
                        "output '{}': include = [] is invalid; \
                         list patterns or omit include entirely for the catch-all",
                        name
                    )));
                }
                parts.push((name, sub));
            }

            let catchall_count = parts.iter().filter(|(_, s)| s.include.is_none()).count();
            let mut all_runtime_deps = Vec::new();
            for (_, sub) in &parts {
                all_runtime_deps.extend(sub.runtime_deps.iter().cloned());
            }
            all_runtime_deps.sort();
            all_runtime_deps.dedup();
            match catchall_count {
                0 => Ok(OutputSection {
                    outputs: Some(OutputConfig::Multi(parts)),
                    install_scripts: None,
                    backup: None,
                    relations: Relations::default(),
                    runtime_deps: all_runtime_deps,
                }),
                1 => {
                    let (_, catchall) = parts.iter().find(|(_, s)| s.include.is_none()).unwrap();
                    let relations = Relations {
                        replaces: catchall.replaces.clone(),
                        conflicts: catchall.conflicts.clone(),
                        provides: catchall.provides.clone(),
                    };
                    let install_scripts = catchall.hooks.as_ref().map(|h| InstallScripts {
                        pre_install: h.pre_install.clone(),
                        post_install: h.post_install.clone(),
                        post_upgrade: h.post_upgrade.clone(),
                        pre_remove: h.pre_remove.clone(),
                        post_remove: h.post_remove.clone(),
                    });
                    let backup_cfg = catchall.backup.as_ref().map(|files| BackupConfig {
                        files: files.clone(),
                    });

                    Ok(OutputSection {
                        outputs: Some(OutputConfig::Multi(parts)),
                        install_scripts,
                        backup: backup_cfg,
                        relations,
                        runtime_deps: all_runtime_deps,
                    })
                }
                _ => Err(WrightError::ParseError(
                    "multiple [[output]] entries have no 'include'; \
                         exactly one catch-all is allowed"
                        .to_string(),
                )),
            }
        }

        // [output] table mode was removed in favor of the single `[[output]]`
        // representation. This keeps single-output and split-output metadata
        // on one schema.
        Some(toml::Value::Table(_)) => Err(WrightError::ParseError(
            "[output] table syntax is no longer supported; use [[output]] instead".to_string(),
        )),

        None => Ok(OutputSection {
            outputs: if main_hooks.is_some() {
                return Err(WrightError::ParseError(
                    "top-level [hooks] is no longer supported; declare hooks inside [[output]]"
                        .to_string(),
                ));
            } else {
                None
            },
            install_scripts: None,
            backup: None,
            relations: Relations::default(),
            runtime_deps: Vec::new(),
        }),

        Some(_) => Err(WrightError::ParseError(
            "output must use [[output]] array-of-tables".to_string(),
        )),
    }
}

impl PlanManifest {
    pub fn parse(content: &str) -> Result<Self> {
        let raw: RawManifest = toml::from_str(content)?;
        let RawManifest {
            plan: metadata,
            build_deps,
            link_deps,
            sources: raw_sources,
            options,
            lifecycle: raw_lifecycle,
            lifecycle_order,
            hooks,
            output,
            discard,
        } = raw;

        let sources = match raw_sources {
            Some(toml::Value::Array(arr)) => {
                let mut entries = Vec::new();
                for (i, val) in arr.into_iter().enumerate() {
                    let entry: Source = val.try_into().map_err(|e: toml::de::Error| {
                        WrightError::ParseError(format!(
                            "failed to parse [[sources]] entry {}: {}",
                            i, e
                        ))
                    })?;
                    entries.push(entry);
                }
                Sources { entries }
            }
            Some(toml::Value::Table(_)) => {
                return Err(WrightError::ParseError(
                    "sources must use [[sources]] array-of-tables".to_string(),
                ));
            }
            None => Sources::default(),
            _ => {
                return Err(WrightError::ParseError(
                    "sources must be an array-of-tables ([[sources]])".to_string(),
                ));
            }
        };

        let mut lifecycle_stages: HashMap<String, LifecycleStage> = HashMap::new();
        if let Some(raw_lifecycle) = raw_lifecycle {
            for (key, value) in raw_lifecycle {
                let stage: LifecycleStage = value.try_into().map_err(|e: toml::de::Error| {
                    WrightError::ParseError(format!(
                        "failed to parse lifecycle stage '{}': {}",
                        key, e
                    ))
                })?;
                lifecycle_stages.insert(key, stage);
            }
        }

        let output_section = parse_output_section(&metadata.name, output, hooks)?;
        let OutputSection {
            outputs,
            install_scripts,
            backup,
            relations,
            runtime_deps,
        } = output_section;

        let manifest = PlanManifest {
            metadata,
            build_deps,
            link_deps,
            runtime_deps,
            relations,
            sources,
            options,
            lifecycle: lifecycle_stages,
            lifecycle_order,
            mvp: None,
            outputs,
            discard,
            install_scripts,
            backup,
            source_plan: None,
        };

        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            WrightError::ParseError(format!("failed to read {}: {}", path.display(), e))
        })?;
        let mut manifest = Self::parse(&content)?;
        manifest.validate()?;

        if path.file_name().and_then(|s| s.to_str()) == Some("plan.toml") {
            let mvp_path = path.with_file_name("mvp.toml");
            if mvp_path.exists() {
                let mvp_content = std::fs::read_to_string(&mvp_path).map_err(|e| {
                    WrightError::ParseError(format!("failed to read {}: {}", mvp_path.display(), e))
                })?;
                let overlay: PhaseConfig = toml::from_str(&mvp_content).map_err(|e| {
                    WrightError::ParseError(format!(
                        "failed to parse {}: {}",
                        mvp_path.display(),
                        e
                    ))
                })?;
                manifest.mvp = Some(overlay);
            }
        }

        Ok(manifest)
    }
}

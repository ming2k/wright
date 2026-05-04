use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, WrightError};

use super::{
    convert::{fabricate_backup, fabricate_install_scripts},
    BackupConfig, FabricateHooks, FabricateOutput, InstallScripts, LifecycleOrder, LifecycleStage,
    OutputConfig, PhaseConfig, PlanManifest, PlanMetadata, Relations, Source, Sources,
};
use super::BuildOptions;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawManifest {
    #[serde(flatten)]
    pub plan: PlanMetadata,
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub link: Vec<String>,
    #[serde(default)]
    pub sources: Option<toml::Value>,
    #[serde(default)]
    pub options: BuildOptions,
    #[serde(default)]
    pub lifecycle: Option<HashMap<String, toml::Value>>,
    #[serde(default)]
    pub lifecycle_order: Option<LifecycleOrder>,
    /// Top-level [hooks] — only valid for single-output plans.
    #[serde(default)]
    pub hooks: Option<FabricateHooks>,
    #[serde(default)]
    pub output: Option<toml::Value>,
}

fn extract_output_string_list(table: &mut toml::value::Table, key: &str) -> Result<Vec<String>> {
    match table.remove(key) {
        Some(toml::Value::Array(arr)) => arr
            .into_iter()
            .map(|v| match v {
                toml::Value::String(s) => Ok(s),
                _ => Err(WrightError::ParseError(format!(
                    "[output].{} entries must be strings",
                    key
                ))),
            })
            .collect(),
        Some(_) => Err(WrightError::ParseError(format!(
            "[output].{} must be an array of strings",
            key
        ))),
        None => Ok(Vec::new()),
    }
}

struct OutputSection {
    outputs: Option<OutputConfig>,
    install_scripts: Option<InstallScripts>,
    backup: Option<BackupConfig>,
    relations: Relations,
}

fn parse_output_section(
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
                    Some(toml::Value::String(s)) => s,
                    Some(_) => {
                        return Err(WrightError::ParseError(format!(
                            "[[output]] entry {}: 'name' must be a string",
                            i
                        )))
                    }
                    None => {
                        return Err(WrightError::ParseError(format!(
                            "[[output]] entry {}: 'name' is required",
                            i
                        )))
                    }
                };
                let sub: super::SubFabricateOutput =
                    toml::Value::Table(table).try_into().map_err(|e: toml::de::Error| {
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
            match catchall_count {
                0 => {
                    // No catch-all: un-matched files are discarded.
                    Ok(OutputSection {
                        outputs: Some(OutputConfig::Multi(parts)),
                        install_scripts: None,
                        backup: None,
                        relations: Relations::default(),
                    })
                }
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
                    let backup_cfg = catchall
                        .backup
                        .as_ref()
                        .map(|files| BackupConfig { files: files.clone() });

                    Ok(OutputSection {
                        outputs: Some(OutputConfig::Multi(parts)),
                        install_scripts,
                        backup: backup_cfg,
                        relations,
                    })
                }
                _ => {
                    return Err(WrightError::ParseError(
                        "multiple [[output]] entries have no 'include'; \
                         exactly one catch-all is allowed"
                            .to_string(),
                    ))
                }
            }
        }

        // --- Single-output mode: [output] table ---
        Some(toml::Value::Table(mut table)) => {
            if main_hooks.is_some() && table.contains_key("hooks") {
                return Err(WrightError::ParseError(
                    "main part hooks must be declared only once (prefer top-level [hooks])"
                        .to_string(),
                ));
            }

            let hooks = match table.remove("hooks") {
                Some(value) => Some(value.try_into().map_err(|e: toml::de::Error| {
                    WrightError::ParseError(format!("failed to parse [output].hooks: {}", e))
                })?),
                None => main_hooks,
            };

            let backup = match table.remove("backup") {
                Some(value) => Some(value.try_into().map_err(|e: toml::de::Error| {
                    WrightError::ParseError(format!("failed to parse [output].backup: {}", e))
                })?),
                None => None,
            };

            let replaces = extract_output_string_list(&mut table, "replaces")?;
            let conflicts = extract_output_string_list(&mut table, "conflicts")?;
            let provides = extract_output_string_list(&mut table, "provides")?;

            if !table.is_empty() {
                let unexpected: Vec<_> = table.keys().collect();
                return Err(WrightError::ParseError(format!(
                    "[output] has unexpected fields: {}. \
                     For multi-output plans use [[output]] array-of-tables.",
                    unexpected
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }

            let main_relations = Relations {
                replaces,
                conflicts,
                provides,
            };
            let main_output = FabricateOutput { hooks, backup };
            let install_scripts = fabricate_install_scripts(&main_output);
            let backup_cfg = fabricate_backup(&main_output);

            let outputs = if main_output.hooks.is_some() || main_output.backup.is_some() {
                Some(OutputConfig::Single(main_output))
            } else {
                None
            };
            Ok(OutputSection {
                outputs,
                install_scripts,
                backup: backup_cfg,
                relations: main_relations,
            })
        }

        None => Ok(OutputSection {
            outputs: None,
            install_scripts: None,
            backup: None,
            relations: Relations::default(),
        }),

        Some(_) => Err(WrightError::ParseError(
            "[output] must be a table or [[output]] array-of-tables".to_string(),
        )),
    }
}

impl PlanManifest {
    pub fn parse(content: &str) -> Result<Self> {
        let raw: RawManifest = toml::from_str(content)?;
        let RawManifest {
            plan,
            build,
            link,
            sources: raw_sources,
            options,
            lifecycle: raw_lifecycle,
            lifecycle_order,
            hooks,
            output,
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
                // "outputs" is the user-facing alias for the internal "fabricate" stage
                let canonical_key = if key == "outputs" {
                    "fabricate".to_string()
                } else {
                    key
                };
                lifecycle_stages.insert(canonical_key, stage);
            }
        }

        let output_section = parse_output_section(output, hooks)?;
        let OutputSection {
            outputs,
            install_scripts,
            backup,
            relations,
        } = output_section;

        // Aggregate runtime deps from outputs so plan-level dependency
        // resolution (e.g. `wright apply --deps`) sees them.
        let mut runtime_deps = Vec::new();
        if let Some(ref outputs) = outputs {
            match outputs {
                super::OutputConfig::Single(_) => {}
                super::OutputConfig::Multi(parts) => {
                    for (_, sub) in parts {
                        runtime_deps.extend(sub.runtime_deps.iter().cloned());
                    }
                }
            }
        }
        runtime_deps.sort();
        runtime_deps.dedup();

        let manifest = PlanManifest {
            plan,
            build_deps: build,
            link_deps: link,
            runtime_deps,
            relations,
            sources,
            options,
            lifecycle: lifecycle_stages,
            lifecycle_order,
            mvp: None,
            outputs,
            install_scripts,
            backup,
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

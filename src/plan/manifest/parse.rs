use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, WrightError};

use super::{
    convert::{empty_sub_fabricate_output, fabricate_backup, fabricate_install_scripts},
    BackupConfig, FabricateHooks, FabricateOutput, InstallScripts, LifecycleOrder, LifecycleStage,
    OutputConfig, PhaseConfig, PlanManifest, PlanMetadata, Relations, Source, Sources,
};
use super::{BuildOptions, Dependencies};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawManifest {
    #[serde(flatten)]
    pub plan: PlanMetadata,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub sources: Option<toml::Value>,
    #[serde(default)]
    pub options: BuildOptions,
    #[serde(default)]
    pub lifecycle: Option<HashMap<String, toml::Value>>,
    #[serde(default)]
    pub lifecycle_order: Option<LifecycleOrder>,
    #[serde(default)]
    pub mvp: Option<PhaseConfig>,
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
    plan_name: &str,
    output_val: Option<toml::Value>,
    main_hooks: Option<FabricateHooks>,
) -> Result<OutputSection> {
    let mut table = match output_val {
        Some(toml::Value::Table(table)) => table,
        Some(_) => {
            return Err(WrightError::ParseError(
                "[output] must be a table".to_string(),
            ));
        }
        None => toml::value::Table::new(),
    };

    let hooks = match table.remove("hooks") {
        Some(value) => {
            if main_hooks.is_some() {
                return Err(WrightError::ParseError(
                    "main part hooks must be declared only once (prefer top-level [hooks])"
                        .to_string(),
                ));
            }
            Some(value.try_into().map_err(|e: toml::de::Error| {
                WrightError::ParseError(format!("failed to parse [output].hooks: {}", e))
            })?)
        }
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
    let main_relations = Relations {
        replaces: replaces.clone(),
        conflicts: conflicts.clone(),
        provides: provides.clone(),
    };

    let main_output = FabricateOutput { hooks, backup };
    let install_scripts = fabricate_install_scripts(&main_output);
    let backup_cfg = fabricate_backup(&main_output);

    if table.is_empty() {
        let outputs = if main_output.hooks.is_some() || main_output.backup.is_some() {
            Some(OutputConfig::Single(main_output))
        } else {
            None
        };
        return Ok(OutputSection {
            outputs,
            install_scripts,
            backup: backup_cfg,
            relations: main_relations,
        });
    }

    let table_value = toml::Value::Table(table);
    let mut outputs: HashMap<String, super::SubFabricateOutput> =
        table_value.try_into().map_err(|e: toml::de::Error| {
            WrightError::ParseError(format!("failed to parse [output.<name>]: {}", e))
        })?;

    if outputs.contains_key(plan_name) {
        return Err(WrightError::ParseError(format!(
            "main output '{}' must use [output], not [output.{}]",
            plan_name, plan_name
        )));
    }

    outputs.insert(
        plan_name.to_string(),
        empty_sub_fabricate_output(
            main_output.hooks.clone(),
            main_output.backup.clone(),
            replaces,
            conflicts,
            provides,
        ),
    );

    Ok(OutputSection {
        outputs: Some(OutputConfig::Multi(outputs)),
        install_scripts,
        backup: backup_cfg,
        relations: main_relations,
    })
}

impl PlanManifest {
    pub fn parse(content: &str) -> Result<Self> {
        let raw: RawManifest = toml::from_str(content)?;
        let RawManifest {
            plan,
            dependencies,
            sources: raw_sources,
            options,
            lifecycle: raw_lifecycle,
            lifecycle_order,
            mvp,
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
                lifecycle_stages.insert(key, stage);
            }
        }

        let output_section = parse_output_section(&plan.name, output, hooks)?;
        let OutputSection {
            outputs,
            install_scripts,
            backup,
            relations,
        } = output_section;

        let manifest = PlanManifest {
            plan,
            dependencies,
            relations,
            sources,
            options,
            lifecycle: lifecycle_stages,
            lifecycle_order,
            mvp,
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

        if path.file_name().and_then(|s| s.to_str()) == Some("plan.toml") {
            let mvp_path = path.with_file_name("mvp.toml");
            if mvp_path.exists() {
                if manifest.mvp.is_some() {
                    return Err(WrightError::ParseError(format!(
                        "do not mix inline [mvp] in {} with sibling {}",
                        path.display(),
                        mvp_path.display()
                    )));
                }

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

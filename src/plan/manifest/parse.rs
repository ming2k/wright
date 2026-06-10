use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Result, WrightError};

use super::PlanBuildOptions;
use super::{
    BackupConfig, DeployScripts, DiscardRule, FabricateHooks, OutputConfig, PhaseConfig,
    PipelineOrder, PipelineStage, PlanManifest, PlanMetadata, Relations, Source, Sources,
};

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    pub plan: Option<PlanMetadata>,
    #[serde(flatten)]
    pub metadata: RawPlanMetadata,
    #[serde(default)]
    pub build_deps: Vec<String>,
    #[serde(default)]
    pub link_deps: Vec<String>,
    #[serde(default)]
    pub sources: Option<toml::Value>,
    #[serde(default)]
    pub options: PlanBuildOptions,
    #[serde(default)]
    pub pipeline: Option<HashMap<String, toml::Value>>,
    #[serde(default)]
    pub pipeline_order: Option<PipelineOrder>,
    /// Top-level [hooks] — legacy syntax; use [[output]].hooks instead.
    #[serde(default)]
    pub hooks: Option<FabricateHooks>,
    #[serde(default)]
    pub output: Option<toml::Value>,
    #[serde(default)]
    pub discard: Vec<DiscardRule>,
}

#[derive(Deserialize, Default)]
struct RawPlanMetadata {
    pub name: Option<String>,
    pub version: Option<String>,
    pub release: Option<u32>,
    pub epoch: Option<u32>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub arch: Option<String>,
    pub url: Option<String>,
    pub maintainer: Option<String>,
}

impl RawPlanMetadata {
    fn merge(self, other: PlanMetadata) -> PlanMetadata {
        PlanMetadata {
            name: self.name.unwrap_or(other.name),
            version: self.version.or(other.version),
            release: self.release.unwrap_or(other.release),
            epoch: self.epoch.unwrap_or(other.epoch),
            description: self.description.unwrap_or(other.description),
            license: self.license.unwrap_or(other.license),
            arch: self.arch.unwrap_or(other.arch),
            url: self.url.or(other.url),
            maintainer: self.maintainer.or(other.maintainer),
        }
    }
}

struct OutputSection {
    outputs: Option<OutputConfig>,
    deploy_scripts: Option<DeployScripts>,
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
                        )));
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
                        )));
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
                    deploy_scripts: None,
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
                    let deploy_scripts = catchall.hooks.as_ref().map(|h| DeployScripts {
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
                        deploy_scripts,
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
            deploy_scripts: None,
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
            plan: section_plan,
            metadata: flattened_metadata,
            build_deps,
            link_deps,
            sources: raw_sources,
            options,
            pipeline: raw_pipeline,
            pipeline_order,
            hooks,
            output,
            discard,
        } = raw;

        let metadata = if let Some(plan) = section_plan {
            flattened_metadata.merge(plan)
        } else {
            // If no [plan] section, we expect all required fields in the flattened metadata.
            if flattened_metadata.name.is_none() {
                return Err(WrightError::ParseError("missing field `name`".to_string()));
            }
            PlanMetadata {
                name: flattened_metadata.name.unwrap(),
                version: flattened_metadata.version,
                release: flattened_metadata.release.ok_or_else(|| {
                    WrightError::ParseError("missing field `release`".to_string())
                })?,
                epoch: flattened_metadata.epoch.unwrap_or(0),
                description: flattened_metadata.description.ok_or_else(|| {
                    WrightError::ParseError("missing field `description`".to_string())
                })?,
                license: flattened_metadata.license.ok_or_else(|| {
                    WrightError::ParseError("missing field `license`".to_string())
                })?,
                arch: flattened_metadata
                    .arch
                    .ok_or_else(|| WrightError::ParseError("missing field `arch`".to_string()))?,
                url: flattened_metadata.url,
                maintainer: flattened_metadata.maintainer,
            }
        };

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

        let mut pipeline_stages: HashMap<String, PipelineStage> = HashMap::new();
        if let Some(raw_pipeline) = raw_pipeline {
            for (key, value) in raw_pipeline {
                let stage: PipelineStage = value.try_into().map_err(|e: toml::de::Error| {
                    WrightError::ParseError(format!(
                        "failed to parse pipeline stage '{}': {}",
                        key, e
                    ))
                })?;
                pipeline_stages.insert(key, stage);
            }
        }

        let output_section = parse_output_section(&metadata.name, output, hooks)?;
        let OutputSection {
            outputs,
            deploy_scripts,
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
            pipeline: pipeline_stages,
            pipeline_order,
            mvp: None,
            outputs,
            discard,
            deploy_scripts,
            backup,
            source_plan: None,
            plan_checksum: None,
        };

        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            WrightError::ParseError(format!("failed to read {}: {}", path.display(), e))
        })?;
        let mut manifest = Self::parse(&content).map_err(|e| match e {
            WrightError::ParseError(msg) => {
                WrightError::ParseError(format!("{}: {}", path.display(), msg))
            }
            other => other,
        })?;
        manifest.validate().map_err(|e| match e {
            WrightError::ValidationError(msg) => {
                WrightError::ValidationError(format!("{}: {}", path.display(), msg))
            }
            other => other,
        })?;
        manifest.plan_checksum = Some(crate::util::checksum::sha256_bytes(content.as_bytes()));

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

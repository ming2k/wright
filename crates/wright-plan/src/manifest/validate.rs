use std::collections::HashMap;

use crate::error::{Result, WrightError};

use super::{OutputConfig, PipelineStage, PlanManifest};

impl PlanManifest {
    pub fn validate(&self) -> Result<()> {
        let name_re = regex::Regex::new(r"^[a-z0-9][a-z0-9_+.-]*$").unwrap();
        if !name_re.is_match(&self.metadata.name) {
            return Err(WrightError::ValidationError(format!(
                "invalid part name '{}': must match [a-z0-9][a-z0-9_+.-]*",
                self.metadata.name
            )));
        }
        if self.metadata.name.len() > 64 {
            return Err(WrightError::ValidationError(
                "part name must be at most 64 characters".to_string(),
            ));
        }

        // Validate version parses if present
        if let Some(ref ver) = self.metadata.version {
            wright_model::version::Version::parse(ver)?;
        }

        if self.metadata.release == 0 {
            return Err(WrightError::ValidationError(
                "release must be >= 1".to_string(),
            ));
        }

        if self.metadata.description.is_empty() {
            return Err(WrightError::ValidationError(
                "description must not be empty".to_string(),
            ));
        }

        if self.metadata.license.is_empty() {
            return Err(WrightError::ValidationError(
                "license must not be empty".to_string(),
            ));
        }

        if self.metadata.arch.is_empty() {
            return Err(WrightError::ValidationError(
                "arch must not be empty".to_string(),
            ));
        }

        // Validate pipeline stage names
        let stages: Vec<&str> = if let Some(ref order) = self.pipeline_order {
            order.stages.iter().map(|s| s.as_str()).collect()
        } else {
            wright_model::pipeline::DEFAULT_PIPELINE_STAGES.to_vec()
        };
        let mut valid_names = std::collections::HashSet::new();
        for stage in &stages {
            valid_names.insert(stage.to_string());
            valid_names.insert(format!("pre_{}", stage));
            valid_names.insert(format!("post_{}", stage));
        }
        for key in self.pipeline.keys() {
            if !valid_names.contains(key) {
                return Err(WrightError::ValidationError(format!(
                    "unknown pipeline stage '{}'. Valid stages: {}",
                    key,
                    stages
                        .iter()
                        .filter(|s| !["fetch", "verify", "extract"].contains(s))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }

        // Each source entry is self-contained (uri + sha256), no positional check needed

        // Validate output config
        if let Some(ref part) = self.outputs {
            match part {
                OutputConfig::Multi(parts) => {
                    let catchall_count = parts.iter().filter(|(_, s)| s.include.is_none()).count();
                    if catchall_count > 1 {
                        return Err(WrightError::ValidationError(
                            "multiple outputs have no 'include'; exactly one catch-all is allowed"
                                .to_string(),
                        ));
                    }
                    let mut output_names = std::collections::HashSet::new();
                    for (sub_name, sub_part) in parts {
                        if !output_names.insert(sub_name) {
                            return Err(WrightError::ValidationError(format!(
                                "duplicate output name '{}' in plan '{}'",
                                sub_name, self.metadata.name
                            )));
                        }
                        if !name_re.is_match(sub_name) {
                            return Err(WrightError::ValidationError(format!(
                                "invalid output name '{}': must match [a-z0-9][a-z0-9_+.-]*",
                                sub_name
                            )));
                        }
                        if matches!(&sub_part.include, Some(v) if v.is_empty()) {
                            return Err(WrightError::ValidationError(format!(
                                "output '{}': include = [] is invalid; list patterns or omit include for the catch-all",
                                sub_name
                            )));
                        }
                        // Non-catch-all outputs must have a description
                        if sub_part.include.is_some() && sub_part.description.is_none() {
                            return Err(WrightError::ValidationError(format!(
                                "output '{}': description is required for non-catch-all outputs",
                                sub_name
                            )));
                        }
                        if let Some(ref ver) = sub_part.version {
                            wright_model::version::Version::parse(ver)?;
                        }
                        if let Some(ref rel) = sub_part.release
                            && *rel == 0
                        {
                            return Err(WrightError::ValidationError(format!(
                                "output '{}': release must be >= 1",
                                sub_name
                            )));
                        }
                    }
                }
            }
        } else if !self.discard.is_empty() {
            return Err(WrightError::ValidationError(
                "[[discard]] is only valid for multi-output plans".to_string(),
            ));
        }

        for rule in &self.discard {
            if rule.include.is_empty() {
                return Err(WrightError::ValidationError(
                    "[[discard]].include must list at least one pattern".to_string(),
                ));
            }
            if rule.reason.trim().is_empty() {
                return Err(WrightError::ValidationError(
                    "[[discard]].reason must explain why matched files are ignored".to_string(),
                ));
            }
        }

        validate_pipeline_isolation("pipeline", &self.pipeline)?;
        if let Some(mvp) = &self.mvp {
            validate_pipeline_isolation("mvp pipeline", &mvp.pipeline)?;
        }

        // Validate deps: plan:output syntax, optional constraints, no duplicates.
        // Existence of the referenced local plan/output is checked by
        // `wright lint`, where the full local plan index is available.
        let dep_kinds = [
            ("build_deps", &self.build_deps),
            ("link_deps", &self.link_deps),
            ("runtime_deps", &self.runtime_deps),
        ];
        for (kind, deps) in &dep_kinds {
            let mut seen = std::collections::HashSet::new();
            for dep in *deps {
                let trimmed = dep.trim();
                if trimmed.is_empty() {
                    return Err(WrightError::ValidationError(format!(
                        "{} contains an empty entry",
                        kind
                    )));
                }
                wright_model::version::parse_dependency_ref(trimmed)
                    .map_err(|e| WrightError::ValidationError(format!("{} entry: {}", kind, e)))?;
                if !seen.insert(trimmed) {
                    return Err(WrightError::ValidationError(format!(
                        "{} contains duplicate entry '{}'",
                        kind, trimmed
                    )));
                }
            }
        }

        Ok(())
    }
}

fn validate_pipeline_isolation(
    scope: &str,
    pipeline: &HashMap<String, PipelineStage>,
) -> Result<()> {
    for (name, stage) in pipeline {
        let Some(isolation) = stage.isolation.as_deref() else {
            continue;
        };
        if let Err(error) = isolation.parse::<wright_model::isolation::IsolationLevel>() {
            return Err(WrightError::ValidationError(format!(
                "{scope} stage '{name}': invalid isolation level '{isolation}': {error}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_name() {
        let toml_str = r#"
name = "Hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PlanManifest::parse(toml_str).is_err());
    }

    #[test]
    fn test_bad_version() {
        let toml_str = r#"
name = "test"
version = "..."
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PlanManifest::parse(toml_str).is_err());
    }

    #[test]
    fn test_multi_package_missing_description() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"

[[output]]
name = "test-lib"
include = ["/usr/lib/**"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("description is required"));
    }

    #[test]
    fn test_multi_package_invalid_name() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"

[[output]]
name = "BadName"
description = "bad"
include = ["/usr/bin/**"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("invalid output name"));
    }

    #[test]
    fn test_duplicate_output_name_error() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]
name = "test-bin"
description = "test bin"
include = ["/usr/bin/**"]

[[output]]
name = "test-bin"
description = "duplicate"
include = ["/usr/lib/**"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("duplicate output name"));
    }

    #[test]
    fn test_discard_rule_requires_reason() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]
name = "test-bin"
description = "test bin"
include = ["/usr/bin/**"]

[[discard]]
include = ["/usr/share/doc/**"]
reason = ""
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("reason"));
    }

    #[test]
    fn test_parse_pipeline_outputs_rejected() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[pipeline.staging]
script = "true"

[pipeline.outputs]
script = "strip ${STAGING_DIR}/usr/bin/test"
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("unknown pipeline stage 'outputs'"),
            "expected validation error for 'outputs' stage, got: {}",
            msg
        );
    }
}

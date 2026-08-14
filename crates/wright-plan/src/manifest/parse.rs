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
                            WrightError::context(
                                format!("failed to parse [[output]] entry '{}'", name),
                                e,
                            )
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
                        WrightError::context(format!("failed to parse [[sources]] entry {}", i), e)
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
                    WrightError::context(format!("failed to parse pipeline stage '{}'", key), e)
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
            plan_source: None,
        };

        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| WrightError::context(format!("failed to read {}", path.display()), e))?;
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
        manifest.plan_checksum = Some(crate::checksum::sha256_bytes(content.as_bytes()));
        manifest.plan_source = Some(content);

        if path.file_name().and_then(|s| s.to_str()) == Some("plan.toml") {
            let mvp_path = path.with_file_name("mvp.toml");
            if mvp_path.exists() {
                let mvp_content = std::fs::read_to_string(&mvp_path).map_err(|e| {
                    WrightError::context(format!("failed to read {}", mvp_path.display()), e)
                })?;
                let overlay: PhaseConfig = toml::from_str(&mvp_content).map_err(|e| {
                    WrightError::context(format!("failed to parse {}", mvp_path.display()), e)
                })?;
                manifest.mvp = Some(overlay);
            }
        }

        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_file_records_plan_source_and_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.toml");
        let content = r#"
name = "snapshot-demo"
version = "1.0.0"
release = 1
description = "snapshot demo"
license = "MIT"
arch = "x86_64"

[pipeline.staging]
executor = "shell"
isolation = "none"
script = "true"
"#;
        std::fs::write(&path, content).unwrap();

        let manifest = PlanManifest::from_file(&path).unwrap();
        assert_eq!(manifest.plan_source.as_deref(), Some(content));
        assert_eq!(
            manifest.plan_checksum.as_deref(),
            Some(crate::checksum::sha256_bytes(content.as_bytes())).as_deref()
        );

        // String-parsed manifests carry neither checksum nor source.
        let parsed = PlanManifest::parse(content).unwrap();
        assert!(parsed.plan_source.is_none());
        assert!(parsed.plan_checksum.is_none());
    }

    #[test]
    fn test_parse_hello_fixture() {
        let toml_str = r#"
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test part"
license = "MIT"
arch = "x86_64"

[pipeline.prepare]
executor = "shell"
isolation = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[pipeline.compile]
executor = "shell"
isolation = "none"
script = """
gcc -o hello hello.c
"""

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 hello ${STAGING_DIR}/usr/bin/hello
"""
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.metadata.name, "hello");
        assert_eq!(manifest.metadata.version.as_deref(), Some("1.0.0"));
        assert_eq!(manifest.metadata.release, 1);
        assert_eq!(manifest.metadata.arch, "x86_64");
        assert_eq!(manifest.metadata.epoch, 0);
        assert!(manifest.pipeline.contains_key("prepare"));
        assert!(manifest.pipeline.contains_key("compile"));
        assert!(manifest.pipeline.contains_key("staging"));
    }

    #[test]
    fn test_parse_full_featured() {
        let toml_str = r#"
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"
arch = "x86_64"
url = "https://nginx.org"
maintainer = "Test <test@test.com>"

link_deps = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]

[[sources]]
type = "http"
url = "https://nginx.org/download/nginx-1.25.3.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"

[[sources]]
type = "local"
path = "patches/fix-headers.patch"

[options]
static = false
debug = false
ccache = true

[pipeline.prepare]
executor = "shell"
isolation = "strict"
script = """
cd ${BUILD_DIR}
patch -Np1 < ${WORKDIR}/fix-headers.patch
"""

[pipeline.configure]
executor = "shell"
isolation = "strict"
env = { CFLAGS = "-O2 -pipe" }
script = """
cd ${BUILD_DIR}
./configure --prefix=/usr
"""

[pipeline.compile]
executor = "shell"
isolation = "strict"
script = """
cd ${BUILD_DIR}
make
"""

[pipeline.check]
executor = "shell"
isolation = "strict"
optional = true
script = """
cd ${BUILD_DIR}
make test
"""

[pipeline.staging]
executor = "shell"
isolation = "strict"
script = """
cd ${BUILD_DIR}
make DESTDIR=${STAGING_DIR} install
"""

[[output]]
conflicts = ["apache"]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]

[output.hooks]
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.metadata.name, "nginx");
        assert_eq!(manifest.metadata.url.as_deref(), Some("https://nginx.org"));
        assert!(manifest.runtime_deps.is_empty());
        assert_eq!(manifest.relations.conflicts, vec!["apache"]);
        assert_eq!(manifest.sources.entries.len(), 2);
        assert!(!manifest.options.static_);
        assert!(manifest.pipeline.contains_key("check"));

        let scripts = manifest.deploy_scripts.as_ref().unwrap();
        assert!(scripts.post_install.is_some());
        assert!(scripts.pre_remove.is_some());

        let backup = manifest.backup.as_ref().unwrap();
        assert_eq!(backup.files.len(), 2);

        // Output config
        match manifest.outputs {
            Some(OutputConfig::Multi(ref outputs)) => {
                let (_, output) = outputs.iter().find(|(name, _)| name == "nginx").unwrap();
                let hooks = output.hooks.as_ref().unwrap();
                assert!(hooks.post_install.is_some());
                assert!(hooks.pre_remove.is_some());
                assert_eq!(output.backup.as_ref().unwrap().len(), 2);
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_parse_with_plan_section() {
        let toml_str = r#"
[plan]
name = "hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.metadata.name, "hello");
        assert_eq!(manifest.metadata.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn test_parse_with_mixed_metadata() {
        let toml_str = r#"
name = "overridden"
[plan]
name = "hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        // Top-level should override [plan] section in my merge implementation
        assert_eq!(manifest.metadata.name, "overridden");
        assert_eq!(manifest.metadata.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn test_missing_name() {
        let toml_str = r#"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PlanManifest::parse(toml_str).is_err());
    }

    #[test]
    fn test_multi_output_include_empty_error() {
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
description = "test lib"
include = []
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("include = []"));
    }

    #[test]
    fn test_multi_output_no_catchall_ok() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"
description = "test bin"
include = ["/usr/bin/**"]

[[output]]
name = "test-lib"
description = "test lib"
include = ["/usr/lib/**"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(parts)) => {
                assert_eq!(parts.len(), 2);
                assert!(parts[0].1.include.is_some());
                assert!(parts[1].1.include.is_some());
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_output_name_defaults_to_plan_name() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]
description = "test output"
include = ["/usr/bin/**"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(parts)) => {
                assert_eq!(parts[0].0, "test");
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_empty_output_name_defaults_to_plan_name() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]
name = ""
description = "test output"
include = ["/usr/bin/**"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(parts)) => {
                assert_eq!(parts[0].0, "test");
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_output_table_is_rejected() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[output]
runtime_deps = ["zlib"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("[output] table syntax"));
    }

    #[test]
    fn test_multi_output_multiple_catchall_error() {
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
description = "test lib"
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("catch-all"));
    }

    #[test]
    fn test_explicit_single_output_ok() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]
name = "test-lib"
description = "test lib"
runtime_deps = ["openssl"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.metadata.name, "test");
        assert_eq!(manifest.runtime_deps, vec!["openssl"]);
        match manifest.outputs {
            Some(OutputConfig::Multi(parts)) => {
                assert_eq!(parts.len(), 1);
                assert_eq!(parts[0].0, "test-lib");
                assert_eq!(parts[0].1.runtime_deps, vec!["openssl"]);
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_multi_output_order_preserved() {
        let toml_str = r#"
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"


[[output]]
name = "gcc-libs"
description = "GCC runtime libraries"
include = ["/usr/lib/lib*.so*"]

[[output]]
name = "gcc-dev"
description = "GCC development files"
include = ["/usr/include/**", "/usr/lib/lib*.a"]

[[output]]
name = "gcc"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                assert_eq!(parts.len(), 3);
                assert_eq!(parts[0].0, "gcc-libs");
                assert_eq!(parts[1].0, "gcc-dev");
                assert_eq!(parts[2].0, "gcc");
                // gcc is the catch-all
                assert!(parts[2].1.include.is_none());
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn test_single_package_with_hooks_and_backup() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[pipeline.staging]
script = "make DESTDIR=${STAGING_DIR} install"

[[output]]
backup = ["/etc/test.conf"]

[output.hooks]
pre_install = "echo pre"
post_install = "ldconfig"
pre_remove = "systemctl stop test"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref outputs)) => {
                let (_, output) = outputs.iter().find(|(name, _)| name == "test").unwrap();
                let hooks = output.hooks.as_ref().unwrap();
                assert_eq!(hooks.pre_install.as_deref(), Some("echo pre"));
                assert_eq!(hooks.post_install.as_deref(), Some("ldconfig"));
                assert_eq!(hooks.pre_remove.as_deref(), Some("systemctl stop test"));
                assert_eq!(output.backup.as_ref().unwrap(), &["/etc/test.conf"]);
            }
            _ => panic!("expected Multi output config"),
        }
        assert!(manifest.deploy_scripts.is_some());
        assert!(manifest.backup.is_some());
    }

    #[test]
    fn test_from_file_loads_sibling_mvp_toml() {
        let dir = tempfile::tempdir().unwrap();
        let plan_path = dir.path().join("plan.toml");
        let mvp_path = dir.path().join("mvp.toml");

        std::fs::write(
            &plan_path,
            r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#,
        )
        .unwrap();

        std::fs::write(
            &mvp_path,
            r#"
build_deps = ["gcc"]

[pipeline.configure]
script = "echo mvp"
"#,
        )
        .unwrap();

        let manifest = PlanManifest::from_file(&plan_path).unwrap();
        let mvp = manifest.mvp.as_ref().unwrap();
        assert_eq!(mvp.build_deps, vec!["gcc"]);
        assert_eq!(
            mvp.pipeline
                .get("configure")
                .map(|stage| stage.script.as_str()),
            Some("echo mvp")
        );
    }

    #[test]
    fn test_defaults() {
        let toml_str = r#"
name = "minimal"
version = "1.0.0"
release = 1
description = "minimal part"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert!(manifest.runtime_deps.is_empty());
        assert!(manifest.build_deps.is_empty());
        assert!(manifest.sources.entries.is_empty());
        assert!(manifest.pipeline.is_empty());
        assert!(manifest.deploy_scripts.is_none());
        assert!(manifest.backup.is_none());
        assert!(!manifest.options.skip_fhs_check);
        assert_eq!(manifest.metadata.epoch, 0);
    }

    #[test]
    fn omitted_stage_isolation_remains_inherited() {
        let manifest = PlanManifest::parse(
            r#"
name = "inherited-isolation"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[pipeline.compile]
script = "true"
"#,
        )
        .unwrap();

        assert_eq!(manifest.pipeline["compile"].isolation, None);
    }

    #[test]
    fn test_skip_fhs_check_option() {
        let toml_str = r#"
name = "kmod"
version = "1.0.0"
release = 1
description = "kernel module"
license = "GPL-2.0"
arch = "x86_64"

[options]
skip_fhs_check = true
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert!(manifest.options.skip_fhs_check);
    }

    #[test]
    fn test_skip_elf_lint_option() {
        let toml_str = r#"
name = "static-tool"
version = "1.0.0"
release = 1
description = "static binary"
license = "MIT"
arch = "x86_64"

[options]
skip_elf_lint = true
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert!(manifest.options.skip_elf_lint);
    }

    #[test]
    fn test_parse_output_relations() {
        let toml_str = r#"
name = "nginx"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]
replaces = ["old-nginx"]
conflicts = ["apache"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.relations.replaces, vec!["old-nginx"]);
        assert_eq!(manifest.relations.conflicts, vec!["apache"]);
    }

    #[test]
    fn test_parse_sources_array() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "https://example.com/foo.tar.gz"
sha256 = "abc123"

[[sources]]
type = "local"
path = "patches/fix.patch"

[[sources]]
type = "git"
url = "https://github.com/foo/bar.git"
ref = "v1.0"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.sources.entries.len(), 3);

        if let Source::Http(http) = &manifest.sources.entries[0] {
            assert_eq!(http.url, "https://example.com/foo.tar.gz");
            assert_eq!(http.sha256, "abc123");
        } else {
            panic!("Expected Http source");
        }

        if let Source::Local(local) = &manifest.sources.entries[1] {
            assert_eq!(local.path, "patches/fix.patch");
        } else {
            panic!("Expected Local source");
        }

        if let Source::Git(git) = &manifest.sources.entries[2] {
            assert_eq!(git.url, "https://github.com/foo/bar.git");
            assert_eq!(git.r#ref, Some("v1.0".to_string()));
        } else {
            panic!("Expected Git source");
        }
    }

    #[test]
    fn test_parse_pre_install_hook() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]

[output.hooks]
pre_install = "echo preparing"
post_install = "ldconfig"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref outputs)) => {
                let (_, output) = outputs.iter().find(|(name, _)| name == "test").unwrap();
                let hooks = output.hooks.as_ref().unwrap();
                assert_eq!(hooks.pre_install.as_deref(), Some("echo preparing"));
                assert_eq!(hooks.post_install.as_deref(), Some("ldconfig"));
            }
            _ => panic!("expected Multi"),
        }
        let scripts = manifest.deploy_scripts.as_ref().unwrap();
        assert_eq!(scripts.pre_install.as_deref(), Some("echo preparing"));
    }
}

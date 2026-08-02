//! Static diagnostics for plan style and maintainability.

use super::manifest::{PlanManifest, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintLevel {
    Warning,
    Help,
}

impl std::fmt::Display for LintLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LintLevel::Warning => write!(f, "warning"),
            LintLevel::Help => write!(f, "help"),
        }
    }
}

/// A single diagnostic reported by static analysis of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintDiagnostic {
    pub code: &'static str,
    pub level: LintLevel,
    pub message: String,
    pub help: Option<String>,
}

/// Analyze a parsed plan and return actionable diagnostics.
pub fn lint_manifest(manifest: &PlanManifest) -> Vec<LintDiagnostic> {
    let mut diagnostics = Vec::new();

    lint_version_variables(manifest, &mut diagnostics);
    lint_hardcoded_system_paths(manifest, &mut diagnostics);
    lint_extract_to_paths(manifest, &mut diagnostics);
    lint_metadata_quality(manifest, &mut diagnostics);

    diagnostics
}

/// L001: Check for hardcoded version strings in source locators.
fn lint_version_variables(manifest: &PlanManifest, diagnostics: &mut Vec<LintDiagnostic>) {
    let Some(ref version) = manifest.metadata.version else {
        return;
    };
    let ver_str = version.trim();
    if ver_str.len() < 2 {
        return;
    }

    for (idx, source) in manifest.sources.entries.iter().enumerate() {
        let (kind, url_or_path) = match source {
            Source::Http(s) => ("source URL", s.url.as_str()),
            Source::Git(s) => ("git URL", s.url.as_str()),
            Source::Local(s) => ("local source path", s.path.as_str()),
        };

        if url_or_path.contains(ver_str) && !url_or_path.contains("${VERSION}") {
            diagnostics.push(LintDiagnostic {
                code: "L001",
                level: LintLevel::Warning,
                message: format!(
                    "hardcoded version '{}' found in {} #{}",
                    ver_str,
                    kind,
                    idx + 1
                ),
                help: Some(format!(
                    "use '${{VERSION}}' instead of hardcoding '{}' in {}",
                    ver_str, kind
                )),
            });
        }
    }
}

/// L002: Check for hardcoded system paths (/usr/local) in pipeline scripts.
fn lint_hardcoded_system_paths(manifest: &PlanManifest, diagnostics: &mut Vec<LintDiagnostic>) {
    for (stage_name, stage) in &manifest.pipeline {
        if stage.script.contains("/usr/local") {
            diagnostics.push(LintDiagnostic {
                code: "L002",
                level: LintLevel::Warning,
                message: format!(
                    "hardcoded path '/usr/local' found in pipeline stage '{}'",
                    stage_name
                ),
                help: Some(
                    "install under '${STAGING_DIR}/usr' instead of '/usr/local'".to_string(),
                ),
            });
        }
    }
}

/// L003: Check for absolute paths in `extract_to`.
fn lint_extract_to_paths(manifest: &PlanManifest, diagnostics: &mut Vec<LintDiagnostic>) {
    for (idx, source) in manifest.sources.entries.iter().enumerate() {
        let extract_to = match source {
            Source::Http(s) => s.extract_to.as_deref(),
            Source::Git(s) => s.extract_to.as_deref(),
            Source::Local(s) => s.extract_to.as_deref(),
        };

        if let Some(path) = extract_to
            && path.starts_with('/')
        {
            diagnostics.push(LintDiagnostic {
                code: "L003",
                level: LintLevel::Warning,
                message: format!(
                    "absolute path '{}' in 'extract_to' for source #{}: may escape build sandbox",
                    path,
                    idx + 1
                ),
                help: Some("use a path relative to '${WORKDIR}' for 'extract_to'".to_string()),
            });
        }
    }
}

/// L004: Check for placeholder text in metadata (description/license).
fn lint_metadata_quality(manifest: &PlanManifest, diagnostics: &mut Vec<LintDiagnostic>) {
    let desc = manifest.metadata.description.trim().to_lowercase();
    if desc == "todo" || desc == "fixme" || desc == "none" || desc.len() < 5 {
        diagnostics.push(LintDiagnostic {
            code: "L004",
            level: LintLevel::Warning,
            message: format!(
                "placeholder or overly brief description '{}'",
                manifest.metadata.description
            ),
            help: Some("provide a clear, detailed summary of what this part does".to_string()),
        });
    }

    let license = manifest.metadata.license.trim().to_lowercase();
    if license == "todo" || license == "fixme" || license == "unknown" {
        diagnostics.push(LintDiagnostic {
            code: "L004",
            level: LintLevel::Warning,
            message: format!(
                "placeholder license identifier '{}'",
                manifest.metadata.license
            ),
            help: Some(
                "use a standard SPDX license identifier (e.g. 'MIT', 'GPL-3.0-only')".to_string(),
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_hardcoded_version_in_source_url() {
        let toml_str = r#"
[plan]
name = "demo"
version = "1.2.3"
release = 1
description = "Demonstration part"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "https://example.com/demo-1.2.3.tar.gz"
sha256 = "SKIP"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        let diags = lint_manifest(&manifest);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "L001");
        assert!(diags[0].message.contains("hardcoded version '1.2.3'"));
    }

    #[test]
    fn accepts_supported_variables() {
        let toml_str = r#"
[plan]
name = "demo"
version = "1.2.3"
release = 1
description = "Demonstration part with proper description"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "https://example.com/demo-${VERSION}.tar.gz"
sha256 = "SKIP"

[pipeline.compile]
script = "make DESTDIR=${STAGING_DIR} install"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        let diags = lint_manifest(&manifest);
        assert!(diags.is_empty());
    }
}

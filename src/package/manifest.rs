use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{WrightError, Result};

#[derive(Debug, Deserialize, Clone)]
pub struct PackageManifest {
    pub plan: PackageMetadata,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub sources: Sources,
    #[serde(default)]
    pub options: BuildOptions,
    #[serde(default)]
    pub lifecycle: HashMap<String, LifecycleStage>,
    #[serde(default)]
    pub lifecycle_order: Option<LifecycleOrder>,
    #[serde(default)]
    pub install_scripts: Option<InstallScripts>,
    #[serde(default)]
    pub backup: Option<BackupConfig>,
    #[serde(default)]
    pub split: HashMap<String, SplitPackage>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SplitPackage {
    pub description: String,
    pub version: Option<String>,
    pub release: Option<u32>,
    pub arch: Option<String>,
    pub license: Option<String>,
    #[serde(default)]
    pub dependencies: Dependencies,
    pub lifecycle: HashMap<String, LifecycleStage>,
    #[serde(default)]
    pub install_scripts: Option<InstallScripts>,
    #[serde(default)]
    pub backup: Option<BackupConfig>,
}

impl SplitPackage {
    /// Produce a full PackageManifest for archive creation, inheriting from the parent.
    pub fn to_manifest(&self, name: &str, parent: &PackageManifest) -> PackageManifest {
        PackageManifest {
            plan: PackageMetadata {
                name: name.to_string(),
                version: self.version.clone().unwrap_or_else(|| parent.plan.version.clone()),
                release: self.release.unwrap_or(parent.plan.release),
                description: self.description.clone(),
                license: self.license.clone().unwrap_or_else(|| parent.plan.license.clone()),
                arch: self.arch.clone().unwrap_or_else(|| parent.plan.arch.clone()),
                url: parent.plan.url.clone(),
                maintainer: parent.plan.maintainer.clone(),
            },
            dependencies: self.dependencies.clone(),
            sources: Sources::default(),
            options: BuildOptions::default(),
            lifecycle: HashMap::new(),
            lifecycle_order: None,
            install_scripts: self.install_scripts.clone(),
            backup: self.backup.clone(),
            split: HashMap::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub description: String,
    pub license: String,
    pub arch: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub maintainer: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Dependencies {
    #[serde(default)]
    pub runtime: Vec<String>,
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub optional: Vec<OptionalDependency>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OptionalDependency {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Sources {
    #[serde(default)]
    pub uris: Vec<String>,
    #[serde(default)]
    pub sha256: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BuildOptions {
    #[serde(default = "default_true")]
    pub strip: bool,
    #[serde(default, rename = "static")]
    pub static_: bool,
    #[serde(default)]
    pub debug: bool,
    #[serde(default = "default_true")]
    pub ccache: bool,
    #[serde(default)]
    pub jobs: Option<u32>,
    #[serde(default)]
    pub memory_limit: Option<u64>,
    #[serde(default)]
    pub cpu_time_limit: Option<u64>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            strip: true,
            static_: false,
            debug: false,
            ccache: true,
            jobs: None,
            memory_limit: None,
            cpu_time_limit: None,
            timeout: None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Clone)]
pub struct LifecycleStage {
    #[serde(default = "default_executor")]
    pub executor: String,
    #[serde(default = "default_sandbox_level")]
    pub sandbox: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub script: String,
}

fn default_executor() -> String {
    "shell".to_string()
}

fn default_sandbox_level() -> String {
    "strict".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct LifecycleOrder {
    pub stages: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct InstallScripts {
    #[serde(default)]
    pub post_install: Option<String>,
    #[serde(default)]
    pub post_upgrade: Option<String>,
    #[serde(default)]
    pub pre_remove: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BackupConfig {
    #[serde(default)]
    pub files: Vec<String>,
}

impl PackageManifest {
    pub fn from_str(content: &str) -> Result<Self> {
        let manifest: Self = toml::from_str(content)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            WrightError::ParseError(format!("failed to read {}: {}", path.display(), e))
        })?;
        Self::from_str(&content)
    }

    pub fn validate(&self) -> Result<()> {
        let name_re = regex::Regex::new(r"^[a-z0-9][a-z0-9_-]*$").unwrap();
        if !name_re.is_match(&self.plan.name) {
            return Err(WrightError::ValidationError(format!(
                "invalid package name '{}': must match [a-z0-9][a-z0-9_-]*",
                self.plan.name
            )));
        }
        if self.plan.name.len() > 64 {
            return Err(WrightError::ValidationError(
                "package name must be at most 64 characters".to_string(),
            ));
        }

        // Validate version parses
        crate::package::version::Version::parse(&self.plan.version)?;

        if self.plan.release == 0 {
            return Err(WrightError::ValidationError(
                "release must be >= 1".to_string(),
            ));
        }

        if self.plan.description.is_empty() {
            return Err(WrightError::ValidationError(
                "description must not be empty".to_string(),
            ));
        }

        if self.plan.license.is_empty() {
            return Err(WrightError::ValidationError(
                "license must not be empty".to_string(),
            ));
        }

        if self.plan.arch.is_empty() {
            return Err(WrightError::ValidationError(
                "arch must not be empty".to_string(),
            ));
        }

        // Validate lifecycle stage names
        let stages: Vec<&str> = if let Some(ref order) = self.lifecycle_order {
            order.stages.iter().map(|s| s.as_str()).collect()
        } else {
            crate::builder::lifecycle::DEFAULT_STAGES.to_vec()
        };
        let mut valid_names = std::collections::HashSet::new();
        for stage in &stages {
            valid_names.insert(stage.to_string());
            valid_names.insert(format!("pre_{}", stage));
            valid_names.insert(format!("post_{}", stage));
        }
        for key in self.lifecycle.keys() {
            if !valid_names.contains(key) {
                return Err(WrightError::ValidationError(format!(
                    "unknown lifecycle stage '{}'. Valid stages: {}",
                    key,
                    stages.iter()
                        .filter(|s| !["fetch", "verify", "extract"].contains(s))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }

        // sha256 count must match uris count
        if self.sources.sha256.len() != self.sources.uris.len() {
            return Err(WrightError::ValidationError(format!(
                "sha256 count ({}) must match uris count ({})",
                self.sources.sha256.len(),
                self.sources.uris.len()
            )));
        }

        // Validate split packages
        for (split_name, split_pkg) in &self.split {
            if !name_re.is_match(split_name) {
                return Err(WrightError::ValidationError(format!(
                    "invalid split package name '{}': must match [a-z0-9][a-z0-9_-]*",
                    split_name
                )));
            }
            if split_name == &self.plan.name {
                return Err(WrightError::ValidationError(format!(
                    "split package name '{}' must not collide with the main package name",
                    split_name
                )));
            }
            if split_pkg.description.is_empty() {
                return Err(WrightError::ValidationError(format!(
                    "split package '{}': description must not be empty",
                    split_name
                )));
            }
            if !split_pkg.lifecycle.contains_key("package") {
                return Err(WrightError::ValidationError(format!(
                    "split package '{}': lifecycle.package stage is required",
                    split_name
                )));
            }
            if let Some(ref ver) = split_pkg.version {
                crate::package::version::Version::parse(ver)?;
            }
            if let Some(ref rel) = split_pkg.release {
                if *rel == 0 {
                    return Err(WrightError::ValidationError(format!(
                        "split package '{}': release must be >= 1",
                        split_name
                    )));
                }
            }
        }

        Ok(())
    }

    /// Get the archive filename for this package
    pub fn archive_filename(&self) -> String {
        format!(
            "{}-{}-{}-{}.wright.tar.zst",
            self.plan.name, self.plan.version, self.plan.release, self.plan.arch
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hello_fixture() {
        let toml = r#"
[plan]
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test package"
license = "MIT"
arch = "x86_64"

[dependencies]
runtime = []
build = ["gcc"]

[sources]
uris = []
sha256 = []

[lifecycle.prepare]
executor = "shell"
sandbox = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[lifecycle.compile]
executor = "shell"
sandbox = "none"
script = """
gcc -o hello hello.c
"""

[lifecycle.package]
executor = "shell"
sandbox = "none"
script = """
install -Dm755 hello ${PKG_DIR}/usr/bin/hello
"""
"#;
        let manifest = PackageManifest::from_str(toml).unwrap();
        assert_eq!(manifest.plan.name, "hello");
        assert_eq!(manifest.plan.version, "1.0.0");
        assert_eq!(manifest.plan.release, 1);
        assert_eq!(manifest.plan.arch, "x86_64");
        assert_eq!(manifest.dependencies.build, vec!["gcc"]);
        assert!(manifest.lifecycle.contains_key("prepare"));
        assert!(manifest.lifecycle.contains_key("compile"));
        assert!(manifest.lifecycle.contains_key("package"));
    }

    #[test]
    fn test_parse_full_featured() {
        let toml = r#"
[plan]
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"
arch = "x86_64"
url = "https://nginx.org"
maintainer = "Test <test@test.com>"

[dependencies]
runtime = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]
build = ["perl", "gcc", "make"]
optional = [
    { name = "geoip", description = "GeoIP module support" },
]
conflicts = ["apache"]
provides = ["http-server"]

[sources]
uris = [
    "https://nginx.org/download/nginx-1.25.3.tar.gz",
    "patches/fix-headers.patch",
]
sha256 = [
    "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83",
    "SKIP",
]

[options]
strip = true
static = false
debug = false
ccache = true

[lifecycle.prepare]
executor = "shell"
sandbox = "strict"
script = """
cd nginx-${PKG_VERSION}
patch -Np1 < ${FILES_DIR}/fix-headers.patch
"""

[lifecycle.configure]
executor = "shell"
sandbox = "strict"
env = { CFLAGS = "-O2 -pipe" }
script = """
cd nginx-${PKG_VERSION}
./configure --prefix=/usr
"""

[lifecycle.compile]
executor = "shell"
sandbox = "strict"
script = """
cd nginx-${PKG_VERSION}
make
"""

[lifecycle.check]
executor = "shell"
sandbox = "strict"
optional = true
script = """
cd nginx-${PKG_VERSION}
make test
"""

[lifecycle.package]
executor = "shell"
sandbox = "strict"
script = """
cd nginx-${PKG_VERSION}
make DESTDIR=${PKG_DIR} install
"""

[install_scripts]
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"

[backup]
files = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
"#;
        let manifest = PackageManifest::from_str(toml).unwrap();
        assert_eq!(manifest.plan.name, "nginx");
        assert_eq!(manifest.plan.url.as_deref(), Some("https://nginx.org"));
        assert_eq!(manifest.dependencies.runtime.len(), 3);
        assert_eq!(manifest.dependencies.conflicts, vec!["apache"]);
        assert_eq!(manifest.dependencies.provides, vec!["http-server"]);
        assert_eq!(manifest.sources.uris.len(), 2);
        assert!(manifest.options.strip);
        assert!(!manifest.options.static_);
        assert!(manifest.lifecycle.get("check").unwrap().optional);

        let scripts = manifest.install_scripts.as_ref().unwrap();
        assert!(scripts.post_install.is_some());
        assert!(scripts.pre_remove.is_some());

        let backup = manifest.backup.as_ref().unwrap();
        assert_eq!(backup.files.len(), 2);
    }

    #[test]
    fn test_invalid_name() {
        let toml = r#"
[plan]
name = "Hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PackageManifest::from_str(toml).is_err());
    }

    #[test]
    fn test_missing_name() {
        let toml = r#"
[plan]
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PackageManifest::from_str(toml).is_err());
    }

    #[test]
    fn test_bad_version() {
        let toml = r#"
[plan]
name = "test"
version = "..."
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PackageManifest::from_str(toml).is_err());
    }

    #[test]
    fn test_sha256_url_mismatch() {
        let toml = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[sources]
uris = ["http://example.com/foo.tar.gz"]
sha256 = []
"#;
        assert!(PackageManifest::from_str(toml).is_err());
    }

    #[test]
    fn test_archive_filename() {
        let toml = r#"
[plan]
name = "hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PackageManifest::from_str(toml).unwrap();
        assert_eq!(
            manifest.archive_filename(),
            "hello-1.0.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_parse_split_packages() {
        let toml = r#"
[plan]
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[lifecycle.compile]
script = "make -j4"

[lifecycle.package]
script = "make DESTDIR=${PKG_DIR} install"

[split.libstdcpp]
description = "GNU C++ standard library"

[split.libstdcpp.dependencies]
runtime = ["libgcc"]

[split.libstdcpp.lifecycle.package]
script = """
install -Dm755 libstdc++.so ${PKG_DIR}/usr/lib/libstdc++.so
"""
"#;
        let manifest = PackageManifest::from_str(toml).unwrap();
        assert_eq!(manifest.split.len(), 1);
        let split = manifest.split.get("libstdcpp").unwrap();
        assert_eq!(split.description, "GNU C++ standard library");
        assert_eq!(split.dependencies.runtime, vec!["libgcc"]);
        assert!(split.lifecycle.contains_key("package"));

        // Test to_manifest
        let split_manifest = split.to_manifest("libstdcpp", &manifest);
        assert_eq!(split_manifest.plan.name, "libstdcpp");
        assert_eq!(split_manifest.plan.version, "14.2.0");
        assert_eq!(split_manifest.plan.release, 1);
        assert_eq!(split_manifest.plan.arch, "x86_64");
        assert_eq!(split_manifest.plan.license, "GPL-3.0-or-later");
        assert_eq!(split_manifest.plan.description, "GNU C++ standard library");
        assert_eq!(split_manifest.dependencies.runtime, vec!["libgcc"]);
        assert_eq!(
            split_manifest.archive_filename(),
            "libstdcpp-14.2.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_split_inherits_overrides() {
        let toml = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.package]
script = "true"

[split.test-doc]
description = "Documentation for test"
version = "1.0.0-doc"
arch = "any"

[split.test-doc.lifecycle.package]
script = "true"
"#;
        let manifest = PackageManifest::from_str(toml).unwrap();
        let split = manifest.split.get("test-doc").unwrap();
        let split_manifest = split.to_manifest("test-doc", &manifest);
        assert_eq!(split_manifest.plan.version, "1.0.0-doc");
        assert_eq!(split_manifest.plan.arch, "any");
        assert_eq!(split_manifest.plan.license, "MIT"); // inherited
    }

    #[test]
    fn test_split_missing_package_stage() {
        let toml = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[split.test-lib]
description = "A library"

[split.test-lib.lifecycle.compile]
script = "make"
"#;
        let err = PackageManifest::from_str(toml).unwrap_err();
        assert!(err.to_string().contains("lifecycle.package stage is required"));
    }

    #[test]
    fn test_split_invalid_name() {
        let toml = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[split.BadName]
description = "bad"

[split.BadName.lifecycle.package]
script = "true"
"#;
        let err = PackageManifest::from_str(toml).unwrap_err();
        assert!(err.to_string().contains("invalid split package name"));
    }

    #[test]
    fn test_split_name_collides_with_main() {
        let toml = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[split.test]
description = "same name"

[split.test.lifecycle.package]
script = "true"
"#;
        let err = PackageManifest::from_str(toml).unwrap_err();
        assert!(err.to_string().contains("must not collide with the main package name"));
    }

    #[test]
    fn test_defaults() {
        let toml = r#"
[plan]
name = "minimal"
version = "1.0.0"
release = 1
description = "minimal package"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PackageManifest::from_str(toml).unwrap();
        assert!(manifest.dependencies.runtime.is_empty());
        assert!(manifest.dependencies.build.is_empty());
        assert!(manifest.sources.uris.is_empty());
        assert!(manifest.options.strip);
        assert!(manifest.lifecycle.is_empty());
        assert!(manifest.install_scripts.is_none());
        assert!(manifest.backup.is_none());
    }
}

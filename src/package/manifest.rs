use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{WrightError, Result};

#[derive(Debug, Deserialize, Clone)]
pub struct PackageManifest {
    pub package: PackageMetadata,
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
    #[serde(default)]
    pub group: Option<String>,
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
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            strip: true,
            static_: false,
            debug: false,
            ccache: true,
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
        if !name_re.is_match(&self.package.name) {
            return Err(WrightError::ValidationError(format!(
                "invalid package name '{}': must match [a-z0-9][a-z0-9_-]*",
                self.package.name
            )));
        }
        if self.package.name.len() > 64 {
            return Err(WrightError::ValidationError(
                "package name must be at most 64 characters".to_string(),
            ));
        }

        // Validate version parses
        crate::package::version::Version::parse(&self.package.version)?;

        if self.package.release == 0 {
            return Err(WrightError::ValidationError(
                "release must be >= 1".to_string(),
            ));
        }

        if self.package.description.is_empty() {
            return Err(WrightError::ValidationError(
                "description must not be empty".to_string(),
            ));
        }

        if self.package.license.is_empty() {
            return Err(WrightError::ValidationError(
                "license must not be empty".to_string(),
            ));
        }

        if self.package.arch.is_empty() {
            return Err(WrightError::ValidationError(
                "arch must not be empty".to_string(),
            ));
        }

        // sha256 count must match uris count
        if self.sources.sha256.len() != self.sources.uris.len() {
            return Err(WrightError::ValidationError(format!(
                "sha256 count ({}) must match uris count ({})",
                self.sources.sha256.len(),
                self.sources.uris.len()
            )));
        }

        Ok(())
    }

    /// Get the archive filename for this package
    pub fn archive_filename(&self) -> String {
        format!(
            "{}-{}-{}-{}.wright.tar.zst",
            self.package.name, self.package.version, self.package.release, self.package.arch
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hello_fixture() {
        let toml = r#"
[package]
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

[lifecycle.build]
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
        assert_eq!(manifest.package.name, "hello");
        assert_eq!(manifest.package.version, "1.0.0");
        assert_eq!(manifest.package.release, 1);
        assert_eq!(manifest.package.arch, "x86_64");
        assert_eq!(manifest.dependencies.build, vec!["gcc"]);
        assert!(manifest.lifecycle.contains_key("prepare"));
        assert!(manifest.lifecycle.contains_key("build"));
        assert!(manifest.lifecycle.contains_key("package"));
    }

    #[test]
    fn test_parse_full_featured() {
        let toml = r#"
[package]
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"
arch = "x86_64"
url = "https://nginx.org"
maintainer = "Test <test@test.com>"
group = "extra"

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

[lifecycle.build]
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
        assert_eq!(manifest.package.name, "nginx");
        assert_eq!(manifest.package.url.as_deref(), Some("https://nginx.org"));
        assert_eq!(manifest.package.group.as_deref(), Some("extra"));
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
[package]
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
[package]
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
[package]
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
[package]
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
[package]
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
    fn test_defaults() {
        let toml = r#"
[package]
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

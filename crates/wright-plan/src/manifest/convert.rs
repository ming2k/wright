//! Conversion from output declarations to complete manifests.

use std::collections::HashMap;

use super::{
    BackupConfig, DeployScripts, PlanBuildOptions, PlanManifest, PlanMetadata, Relations, Sources,
    SubFabricateOutput,
};

impl SubFabricateOutput {
    /// Produce a full PlanManifest for archive creation, inheriting from the parent.
    pub fn to_manifest(&self, name: &str, parent: &PlanManifest) -> PlanManifest {
        let description = self
            .description
            .clone()
            .unwrap_or_else(|| parent.metadata.description.clone());

        let deploy_scripts = self.hooks.as_ref().map(|h| DeployScripts {
            pre_install: h.pre_install.clone(),
            post_install: h.post_install.clone(),
            post_upgrade: h.post_upgrade.clone(),
            pre_remove: h.pre_remove.clone(),
            post_remove: h.post_remove.clone(),
        });

        let backup = self.backup.as_ref().map(|files| BackupConfig {
            files: files.clone(),
        });

        PlanManifest {
            metadata: PlanMetadata {
                name: name.to_string(),
                version: self
                    .version
                    .clone()
                    .or_else(|| parent.metadata.version.clone()),
                release: self.release.unwrap_or(parent.metadata.release),
                epoch: parent.metadata.epoch,
                description,
                license: self
                    .license
                    .clone()
                    .unwrap_or_else(|| parent.metadata.license.clone()),
                arch: self
                    .arch
                    .clone()
                    .unwrap_or_else(|| parent.metadata.arch.clone()),
                url: parent.metadata.url.clone(),
                maintainer: parent.metadata.maintainer.clone(),
            },
            build_deps: Vec::new(),
            link_deps: Vec::new(),
            runtime_deps: self.runtime_deps.clone(),
            relations: Relations {
                replaces: self.replaces.clone(),
                conflicts: self.conflicts.clone(),
                provides: self.provides.clone(),
            },
            sources: Sources::default(),
            options: PlanBuildOptions::default(),
            pipeline: HashMap::new(),
            pipeline_order: None,
            mvp: None,
            outputs: None,
            discard: Vec::new(),
            deploy_scripts,
            backup,
            source_plan: Some(parent.metadata.name.clone()),
            plan_checksum: parent.plan_checksum.clone(),
            plan_source: parent.plan_source.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::OutputConfig;

    #[test]
    fn sub_manifest_inherits_plan_source() {
        let mut manifest = PlanManifest::parse(
            r#"
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[pipeline.compile]
script = "make -j4"

[[output]]
name = "gcc"

[[output]]
name = "libstdc++"
description = "C++ standard library"
include = ["/usr/lib/libstdc++.so*"]
"#,
        )
        .unwrap();
        manifest.plan_checksum = Some("deadbeef".to_string());
        manifest.plan_source = Some("raw plan text".to_string());

        let Some(OutputConfig::Multi(ref parts)) = manifest.outputs else {
            panic!("expected multi-output manifest");
        };
        let (sub_name, sub_part) = parts.iter().find(|(n, _)| n == "libstdc++").unwrap();
        let sub_manifest = sub_part.to_manifest(sub_name, &manifest);

        assert_eq!(sub_manifest.plan_checksum.as_deref(), Some("deadbeef"));
        assert_eq!(sub_manifest.plan_source.as_deref(), Some("raw plan text"));
    }

    #[test]
    fn test_parse_multi_packages() {
        let toml_str = r#"
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"


[pipeline.compile]
script = "make -j4"

[pipeline.staging]
script = "make DESTDIR=${STAGING_DIR} install"

[[output]]
name = "gcc"

[[output]]
name = "libstdc++"
description = "GNU C++ standard library"
include = ["/usr/lib/libstdc*"]
runtime_deps = ["libgcc"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                assert_eq!(parts.len(), 2);
                let (_, libstdcpp) = parts.iter().find(|(n, _)| n == "libstdc++").unwrap();
                assert_eq!(
                    libstdcpp.description.as_deref(),
                    Some("GNU C++ standard library")
                );
                assert_eq!(libstdcpp.runtime_deps, vec!["libgcc"]);

                let sub_manifest = libstdcpp.to_manifest("libstdc++", &manifest);
                assert_eq!(sub_manifest.metadata.name, "libstdc++");
                assert_eq!(sub_manifest.metadata.version.as_deref(), Some("14.2.0"));
                assert_eq!(sub_manifest.metadata.release, 1);
                assert_eq!(sub_manifest.metadata.arch, "x86_64");
                assert_eq!(sub_manifest.metadata.license, "GPL-3.0-or-later");
                assert_eq!(
                    sub_manifest.metadata.description,
                    "GNU C++ standard library"
                );
                assert_eq!(sub_manifest.runtime_deps, vec!["libgcc"]);
                assert_eq!(
                    sub_manifest.part_filename(),
                    "libstdc++-14.2.0-1-x86_64.wright.tar.zst"
                );
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_multi_package_sub_part_relations() {
        let toml_str = r#"
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP server"
license = "BSD-2-Clause"
arch = "x86_64"


[pipeline.staging]
script = "make DESTDIR=${STAGING_DIR} install"

[[output]]
name = "nginx"
conflicts = ["apache"]
provides = ["http-server"]

[[output]]
name = "nginx-doc"
description = "Nginx documentation files"
provides = ["nginx-documentation"]
include = ["/usr/share/doc/**"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.relations.conflicts, vec!["apache"]);
        assert_eq!(manifest.relations.provides, vec!["http-server"]);

        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                let (_, main) = parts.iter().find(|(n, _)| n == "nginx").unwrap();
                let main_manifest = main.to_manifest("nginx", &manifest);
                assert_eq!(main_manifest.relations.conflicts, vec!["apache"]);
                assert_eq!(main_manifest.relations.provides, vec!["http-server"]);

                let (_, doc) = parts.iter().find(|(n, _)| n == "nginx-doc").unwrap();
                let doc_manifest = doc.to_manifest("nginx-doc", &manifest);
                assert_eq!(doc_manifest.relations.provides, vec!["nginx-documentation"]);
                assert!(doc_manifest.relations.conflicts.is_empty());
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_multi_package_inherits_overrides() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[pipeline.staging]
script = "true"

[[output]]
name = "test"

[[output]]
name = "test-doc"
description = "Documentation for test"
version = "1.0.0-doc"
arch = "any"
include = ["/usr/share/doc/**"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                let (_, doc) = parts.iter().find(|(n, _)| n == "test-doc").unwrap();
                let doc_manifest = doc.to_manifest("test-doc", &manifest);
                assert_eq!(doc_manifest.metadata.version.as_deref(), Some("1.0.0-doc"));
                assert_eq!(doc_manifest.metadata.arch, "any");
                assert_eq!(doc_manifest.metadata.license, "MIT"); // inherited
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_main_package_in_multi_inherits_description() {
        let toml_str = r#"
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"


[pipeline.staging]
script = "make DESTDIR=${STAGING_DIR} install"

[[output]]
name = "gcc"

[[output]]
name = "gcc-doc"
description = "GCC documentation"
include = ["/usr/share/doc/**"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                let (_, main) = parts.iter().find(|(n, _)| n == "gcc").unwrap();
                let main_manifest = main.to_manifest("gcc", &manifest);
                assert_eq!(
                    main_manifest.metadata.description,
                    "The GNU Compiler Collection"
                );
            }
            _ => panic!("expected Multi"),
        }
    }
}

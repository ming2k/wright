use std::collections::HashMap;

use super::{
    BackupConfig, BuildOptions, InstallScripts, PlanManifest, PlanMetadata, Relations, Sources,
    SubFabricateOutput,
};

impl SubFabricateOutput {
    /// Produce a full PlanManifest for archive creation, inheriting from the parent.
    pub fn to_manifest(&self, name: &str, parent: &PlanManifest) -> PlanManifest {
        let description = self
            .description
            .clone()
            .unwrap_or_else(|| parent.metadata.description.clone());

        let install_scripts = self.hooks.as_ref().map(|h| InstallScripts {
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
            options: BuildOptions::default(),
            lifecycle: HashMap::new(),
            lifecycle_order: None,
            mvp: None,
            outputs: None,
            discard: Vec::new(),
            install_scripts,
            backup,
            source_plan: Some(parent.metadata.name.clone()),
        }
    }
}

use std::collections::HashMap;

use super::{
    BackupConfig, BuildOptions, InstallScripts, PlanManifest, PlanMetadata,
    Relations, Sources, SubFabricateOutput,
};

impl SubFabricateOutput {
    /// Produce a full PlanManifest for archive creation, inheriting from the parent.
    pub fn to_manifest(&self, name: &str, parent: &PlanManifest) -> PlanManifest {
        let description = self
            .description
            .clone()
            .unwrap_or_else(|| parent.plan.description.clone());

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

        // Build the output-level dependency set.
        // Plan-level build/link are NOT inherited into the output manifest
        // because they are build-planning metadata, not install-time metadata.
        let output_runtime = if !self.runtime_deps.is_empty() {
            &self.runtime_deps
        } else {
            &self.dependencies.runtime
        };
        let output_optional = if !self.optional_deps.is_empty() {
            &self.optional_deps
        } else {
            &self.dependencies.optional
        };

        let mut merged_deps = parent.dependencies.clone();
        // Output-level runtime overrides plan-level runtime
        if !output_runtime.is_empty() {
            merged_deps.runtime = output_runtime.clone();
        }
        // Output-level optional overrides plan-level optional
        if !output_optional.is_empty() {
            merged_deps.optional = output_optional.clone();
        }
        // Do NOT inherit build/link from parent into the output manifest
        merged_deps.build.clear();
        merged_deps.link.clear();

        PlanManifest {
            plan: PlanMetadata {
                name: name.to_string(),
                version: self
                    .version
                    .clone()
                    .or_else(|| parent.plan.version.clone()),
                release: self.release.unwrap_or(parent.plan.release),
                epoch: parent.plan.epoch,
                description,
                license: self
                    .license
                    .clone()
                    .unwrap_or_else(|| parent.plan.license.clone()),
                arch: self
                    .arch
                    .clone()
                    .unwrap_or_else(|| parent.plan.arch.clone()),
                url: parent.plan.url.clone(),
                maintainer: parent.plan.maintainer.clone(),
            },
            dependencies: merged_deps,
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
            install_scripts,
            backup,
        }
    }
}

pub(super) fn fabricate_install_scripts(output: &super::FabricateOutput) -> Option<InstallScripts> {
    output.hooks.as_ref().map(|h| InstallScripts {
        pre_install: h.pre_install.clone(),
        post_install: h.post_install.clone(),
        post_upgrade: h.post_upgrade.clone(),
        pre_remove: h.pre_remove.clone(),
        post_remove: h.post_remove.clone(),
    })
}

pub(super) fn fabricate_backup(output: &super::FabricateOutput) -> Option<BackupConfig> {
    output.backup.as_ref().map(|files| BackupConfig {
        files: files.clone(),
    })
}

use std::collections::HashMap;

use super::{
    default_dockyard_level, default_executor, BackupConfig, BuildOptions, Dependencies,
    FabricateHooks, InstallScripts, PlanManifest, PlanMetadata, Relations, Sources,
    SubFabricateOutput,
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

        PlanManifest {
            plan: PlanMetadata {
                name: name.to_string(),
                version: self
                    .version
                    .clone()
                    .unwrap_or_else(|| parent.plan.version.clone()),
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
            dependencies: self.dependencies.clone(),
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
            fabricate: None,
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

pub(super) fn empty_sub_fabricate_output(
    hooks: Option<FabricateHooks>,
    backup: Option<Vec<String>>,
    replaces: Vec<String>,
    conflicts: Vec<String>,
    provides: Vec<String>,
) -> SubFabricateOutput {
    SubFabricateOutput {
        description: None,
        version: None,
        release: None,
        arch: None,
        license: None,
        dependencies: Dependencies::default(),
        replaces,
        conflicts,
        provides,
        script: String::new(),
        executor: default_executor(),
        dockyard: default_dockyard_level(),
        env: HashMap::new(),
        hooks,
        backup,
    }
}

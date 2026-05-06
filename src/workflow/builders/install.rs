use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::config::GlobalConfig;
use crate::part::store::LocalPartStore;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::spec::{WorkflowBuilder, WorkflowSpec};
use crate::workflow::steps::{InstallBatchInputs, InstallBatchStep, InstallSource};

#[derive(Serialize)]
struct InstallArchivesInputs {
    archives: Vec<ArchiveInput>,
    explicit_targets: Vec<String>,
    root_dir: PathBuf,
    force: bool,
    nodeps: bool,
}

#[derive(Serialize)]
struct ArchiveInput {
    path: PathBuf,
    hash: String,
}

/// `wright install --path …` — install pre-built archives directly.
pub fn build_install_archives_workflow(
    archive_paths: Vec<PathBuf>,
    explicit_targets: Vec<String>,
    root_dir: PathBuf,
    part_store: Arc<LocalPartStore>,
    force: bool,
    nodeps: bool,
) -> Result<WorkflowSpec> {
    let mut sorted_archives: Vec<(PathBuf, String)> = archive_paths
        .into_iter()
        .map(|p| {
            let h = crate::util::checksum::sha256_file(&p)
                .map_err(|e| WorkflowError::Other(format!("hash archive: {}", e)))?;
            Ok((p, h))
        })
        .collect::<Result<Vec<_>>>()?;
    sorted_archives.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sorted_explicit = explicit_targets;
    sorted_explicit.sort();
    sorted_explicit.dedup();

    let inputs = InstallArchivesInputs {
        archives: sorted_archives
            .iter()
            .map(|(p, h)| ArchiveInput {
                path: p.clone(),
                hash: h.clone(),
            })
            .collect(),
        explicit_targets: sorted_explicit.clone(),
        root_dir: root_dir.clone(),
        force,
        nodeps,
    };

    let mut wfb = WorkflowBuilder::new("install", &inputs)?;
    let sources: Vec<InstallSource> = sorted_archives
        .into_iter()
        .map(|(path, hash)| InstallSource::Path { path, hash })
        .collect();

    wfb.add(InstallBatchStep::new(
        InstallBatchInputs {
            label: "install".to_string(),
            sources,
            explicit_targets: sorted_explicit,
            explicit_pack_files: Vec::new(),
            root_dir,
            force,
            nodeps,
            plans_to_reconcile: Vec::new(),
        },
        Vec::new(),
        part_store,
    ))?;

    Ok(wfb.build())
}

/// `wright install <plan-name>` — resolve plan output archives from
/// `parts_dir` and install them. No build, no package.
pub fn build_install_targets_workflow(
    config: &GlobalConfig,
    targets: Vec<String>,
    root_dir: PathBuf,
    part_store: Arc<LocalPartStore>,
    force: bool,
    nodeps: bool,
) -> Result<WorkflowSpec> {
    let plan_dirs = crate::builder::orchestrator::plan_search_dirs(config);
    let all_plans = crate::plan::discovery::get_all_plans(&plan_dirs)
        .map_err(|e| WorkflowError::Other(format!("discover plans: {}", e)))?;

    let mut archive_paths: Vec<PathBuf> = Vec::new();
    let mut explicit_targets: Vec<String> = Vec::new();

    for target in &targets {
        let one = vec![target.clone()];
        let plan_paths = crate::builder::orchestrator::resolve_targets(&one, &all_plans, &plan_dirs)
            .map_err(|e| WorkflowError::Other(format!("resolve {}: {}", target, e)))?;
        for plan_path in plan_paths {
            let manifest = PlanManifest::from_file(&plan_path)
                .map_err(|e| WorkflowError::Other(format!("read plan: {}", e)))?;
            for (name, path) in archive_entries(&manifest, &config.general.parts_dir) {
                if !path.exists() {
                    return Err(WorkflowError::Other(format!(
                        "expected archive missing: {}",
                        path.display()
                    )));
                }
                archive_paths.push(path);
                explicit_targets.push(name);
            }
        }
    }

    build_install_archives_workflow(
        archive_paths,
        explicit_targets,
        root_dir,
        part_store,
        force,
        nodeps,
    )
}

fn archive_entries(
    manifest: &PlanManifest,
    parts_dir: &std::path::Path,
) -> Vec<(String, PathBuf)> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref outputs)) => outputs
            .iter()
            .map(|(sub_name, sub_part)| {
                let sub_manifest = sub_part.to_manifest(sub_name, manifest);
                (
                    sub_name.clone(),
                    parts_dir.join(sub_manifest.part_filename()),
                )
            })
            .collect(),
        _ => vec![(
            manifest.metadata.name.clone(),
            parts_dir.join(manifest.part_filename()),
        )],
    }
}

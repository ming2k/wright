use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::part::pack::{self, PackOrigin};
use crate::part::store::LocalPartStore;
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::spec::{WorkflowBuilder, WorkflowSpec};
use crate::workflow::steps::{
    ApplyConfigInputs, ApplyConfigStep, ApplyOverlayInputs, ApplyOverlayStep, ExtractPackInputs,
    ExtractPackStep, InstallBatchInputs, InstallBatchStep, InstallSource,
};

#[derive(Serialize)]
pub struct LaunchPackInputs {
    pub pack_path: PathBuf,
    pub pack_sha256: String,
    pub root_dir: PathBuf,
    pub force: bool,
}

/// Construct the workflow for `wright launch <pack>`:
/// extract → install batch → overlay → config.
pub fn build_launch_pack_workflow(
    pack_path: PathBuf,
    root_dir: PathBuf,
    staging_root: PathBuf,
    part_store: Arc<LocalPartStore>,
    force: bool,
) -> Result<WorkflowSpec> {
    let pack_sha256 = crate::util::checksum::sha256_file(&pack_path)
        .map_err(|e| WorkflowError::Other(format!("hash pack: {}", e)))?;

    // Read the manifest at construction time so we can wire the install
    // sources (file names) and apply-config (typed fields) statically. The
    // pack's content hash is the source of truth for workflow identity, so
    // identical packs at different paths still resume each other.
    let manifest = pack::read_manifest(&pack_path)
        .map_err(|e| WorkflowError::Other(format!("read pack: {}", e)))?;

    let mut sorted_files: Vec<(String, bool)> = manifest
        .parts
        .iter()
        .map(|p| (p.file.clone(), matches!(p.origin, PackOrigin::Manual)))
        .collect();
    sorted_files.sort();

    let inputs = LaunchPackInputs {
        pack_path: pack_path.clone(),
        pack_sha256: pack_sha256.clone(),
        root_dir: root_dir.clone(),
        force,
    };

    let mut wfb = WorkflowBuilder::new("launch_pack", &inputs)?;

    let extract_id = wfb.add(ExtractPackStep::new(
        ExtractPackInputs {
            pack_sha256: pack_sha256.clone(),
        },
        Vec::new(),
        pack_path,
        staging_root,
    ))?;

    // Determine which part files are user-facing ("manual") so we record
    // them as Origin::Manual in the DB after install. The remainder are
    // dependency-installed.
    let part_files_sorted: Vec<String> = sorted_files.iter().map(|(f, _)| f.clone()).collect();
    let manual_part_files: Vec<String> = sorted_files
        .iter()
        .filter(|(_, m)| *m)
        .map(|(f, _)| f.clone())
        .collect();

    // Pack filenames are stable identifiers in the workflow id; the
    // install step reads `.PARTINFO` from each archive at execute time to
    // map filenames to authoritative part names.
    let mut sorted_manual_files = manual_part_files;
    sorted_manual_files.sort();
    sorted_manual_files.dedup();

    let install_id = wfb.add(InstallBatchStep::new(
        InstallBatchInputs {
            label: "launch-install".to_string(),
            sources: vec![InstallSource::FromPackExtract {
                step: extract_id.clone(),
                files: part_files_sorted,
            }],
            explicit_targets: Vec::new(),
            explicit_pack_files: sorted_manual_files,
            root_dir: root_dir.clone(),
            force,
            nodeps: false,
            plans_to_reconcile: Vec::new(),
        },
        vec![extract_id.clone()],
        part_store,
    ))?;

    if manifest.overlay_sha256.is_some() {
        wfb.add(ApplyOverlayStep::new(
            ApplyOverlayInputs {
                root_dir: root_dir.clone(),
                extract_step: extract_id.clone(),
            },
            vec![extract_id.clone(), install_id.clone()],
        ))?;
    }

    if let Some(cfg) = manifest.config.as_ref() {
        let mut services = cfg.services.clone();
        services.sort();
        services.dedup();
        wfb.add(ApplyConfigStep::new(
            ApplyConfigInputs {
                root_dir,
                hostname: cfg.hostname.clone(),
                timezone: cfg.timezone.clone(),
                locale: cfg.locale.clone(),
                services,
            },
            vec![install_id],
        ))?;
    }

    Ok(wfb.build())
}

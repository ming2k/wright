use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use crate::cli::launch::LaunchArgs;
use crate::commands::workflow_run::{drive_command, DriveOptions};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::part::group::{self, GroupManifest};
use crate::part::store::LocalPartStore;

pub async fn execute_launch(
    args: LaunchArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    if root_dir == Path::new("/") {
        anyhow::bail!(
            "wright launch refuses to fill `/`; pass --root <PATH> pointing at a mounted target root"
        );
    }
    ensure_target_skeleton(root_dir)
        .with_context(|| format!("failed to prepare target root {}", root_dir.display()))?;

    // Isolate build outputs under the target root so the host is not polluted.
    let mut launch_config = config.clone();
    redirect_build_paths_for_target(&mut launch_config,
        root_dir,
    );

    if let Some(group_path) = args.group.clone() {
        let manifest = group::read_manifest(&group_path)
            .with_context(|| format!("read group {}", group_path.display()))?;
        return launch_from_group(
            manifest, args, &launch_config, db_path, root_dir, verbose, quiet,
        )
        .await;
    }

    if let Some(plans_dir) = args.plans.clone() {
        return launch_from_plans(args, plans_dir, &launch_config, db_path, root_dir, verbose, quiet).await;
    }

    anyhow::bail!("wright launch needs --group or --plans; nothing to do")
}

async fn launch_from_group(
    manifest: GroupManifest,
    args: LaunchArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    println!(
        "launching group {} {} into {}",
        manifest.group.name,
        manifest.group.version,
        root_dir.display()
    );
    info!(
        "group carries {} plan(s), {} assumption(s){}",
        manifest.group.plans.len(),
        manifest.assumes.len(),
        if manifest.config.is_some() {
            ", config block"
        } else {
            ""
        }
    );

    if args.dry_run {
        println!();
        println!("[dry-run] would build and install {} plan(s):", manifest.group.plans.len());
        for p in &manifest.group.plans {
            println!("  {}", p);
        }
        if !manifest.assumes.is_empty() {
            println!();
            println!("[dry-run] would assume {} external(s):", manifest.assumes.len());
            for a in &manifest.assumes {
                println!("  {} {}", a.name, a.version);
            }
        }
        return Ok(());
    }

    // Copy plans and groups into the target so the installed system can
    // self-maintain without referring back to the host tree.
    let target_plans_dir = root_dir.join("var/lib/wright/plans");
    let target_groups_dir = root_dir.join("var/lib/wright/groups");
    copy_plans_to_target(&config.general.plans_dir, &target_plans_dir)
        .context("copy host plans into target root")?;
    for extra_dir in &config.general.extra_plans_dirs {
        copy_plans_to_target(extra_dir, &target_plans_dir)
            .context("copy extra plans into target root")?;
    }
    if let Some(group_path) = args.group.as_ref() {
        copy_groups_to_target(&[group_path.clone()], &target_groups_dir)
            .context("copy group manifest into target root")?;
    }
    write_target_wright_toml(root_dir, config)
        .context("write target wright.toml")?;

    // Pre-register external assumptions.
    let db = InstalledDb::open(db_path)
        .await
        .context("failed to open target database")?;
    for assume in &manifest.assumes {
        db.assume_part(&assume.name, &assume.version)
            .await
            .with_context(|| format!("failed to assume {}", assume.name))?;
    }
    drop(db);

    // Apply config (hostname, timezone, locale, services) after install.
    let part_store = setup_part_store(config)?;

    crate::commands::system::apply::execute_apply(crate::commands::system::apply::ApplyArgs {
        targets: manifest.group.plans.clone(),
        fresh: false,
        deps: None,
        rdeps: None,
        match_policies: Vec::new(),
        depth: None,
        force: args.force,
        dry_run: args.dry_run,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store: &part_store,
    })
    .await?;

    // Apply group config if present.
    if let Some(cfg) = manifest.config.as_ref() {
        apply_group_config(root_dir, cfg).await?;
    }

    println!("launched {} -> {}", manifest.group.name, root_dir.display());
    Ok(())
}

async fn launch_from_plans(
    args: LaunchArgs,
    plans_dir: PathBuf,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    if args.plan_targets.is_empty() {
        anyhow::bail!("--plans needs one or more plan or group names to launch");
    }

    let mut launch_config = config.clone();
    launch_config.general.extra_plans_dirs.push(plans_dir.clone());

    let mut plan_dirs: Vec<PathBuf> = vec![launch_config.general.plans_dir.clone()];
    plan_dirs.extend(launch_config.general.extra_plans_dirs.iter().cloned());

    let (targets, group_assumes, group_config) =
        group::expand_group_references(args.plan_targets.clone(), &plan_dirs)?;

    if targets.is_empty() {
        anyhow::bail!("no plans to build after expanding groups");
    }

    // Copy plans and groups into the target so the installed system can
    // self-maintain without referring back to the host tree.
    let target_plans_dir = root_dir.join("var/lib/wright/plans");
    let target_groups_dir = root_dir.join("var/lib/wright/groups");
    copy_plans_to_target(&plans_dir, &target_plans_dir)
        .context("copy source plans into target root")?;
    copy_plans_to_target(&config.general.plans_dir, &target_plans_dir)
        .context("copy host plans into target root")?;
    for extra_dir in &config.general.extra_plans_dirs {
        copy_plans_to_target(extra_dir, &target_plans_dir)
            .context("copy extra plans into target root")?;
    }

    // Collect group files that were referenced and copy them too.
    let mut referenced_groups: Vec<PathBuf> = Vec::new();
    for target in &args.plan_targets {
        if let Some(group_name) = target.strip_prefix('@') {
            if let Some(group_path) = group::find_group_manifest(
                &[plans_dir.clone(), config.general.plans_dir.clone()]
                    .into_iter()
                    .chain(config.general.extra_plans_dirs.iter().cloned())
                    .collect::<Vec<_>>()
                    .as_slice(),
                group_name,
            ) {
                referenced_groups.push(group_path);
            }
        }
    }
    if !referenced_groups.is_empty() {
        copy_groups_to_target(&referenced_groups, &target_groups_dir)
            .context("copy referenced groups into target root")?;
    }
    write_target_wright_toml(root_dir, config)
        .context("write target wright.toml")?;

    let part_store = setup_part_store(config)?;

    // Pre-register any assumptions collected from groups.
    if !group_assumes.is_empty() {
        let db = InstalledDb::open(db_path)
            .await
            .context("failed to open target database")?;
        for assume in &group_assumes {
            db.assume_part(&assume.name, &assume.version)
                .await
                .with_context(|| format!("failed to assume {}", assume.name))?;
        }
        drop(db);
    }

    crate::commands::system::apply::execute_apply(crate::commands::system::apply::ApplyArgs {
        targets,
        fresh: false,
        deps: None,
        rdeps: None,
        match_policies: Vec::new(),
        depth: None,
        force: args.force,
        dry_run: args.dry_run,
        config: &launch_config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store: &part_store,
    })
    .await?;

    if let Some(cfg) = group_config.as_ref() {
        apply_group_config(root_dir, cfg).await?;
    }

    println!("launched plans -> {}", root_dir.display());
    Ok(())
}

fn redirect_build_paths_for_target(config: &mut GlobalConfig, root_dir: &Path) {
    // When targeting an alternate root, build and package outputs should
    // live under that root so the host system is not polluted.
    config.general.parts_dir = root_dir.join("var/lib/wright/parts");
    config.build.build_dir = root_dir.join("var/tmp/wright/workshop");
}

fn setup_part_store(config: &GlobalConfig) -> Result<LocalPartStore> {
    crate::commands::setup_local_part_store(config)
}

fn ensure_target_skeleton(root_dir: &Path) -> std::io::Result<()> {
    for sub in [
        "var/lib/wright",
        "var/lib/wright/parts",
        "var/lib/wright/staging",
        "var/lib/wright/lock",
        "var/lib/wright/plans",
        "var/lib/wright/groups",
        "var/log/wright",
        "etc/wright",
    ] {
        std::fs::create_dir_all(root_dir.join(sub))?;
    }
    Ok(())
}

/// Copy plan directories from a source tree into the target root.
/// Only copies directories that contain a `plan.toml` file.
fn copy_plans_to_target(source_dir: &Path, target_plans_dir: &Path) -> Result<()> {
    if !source_dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(source_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let plan_toml = path.join("plan.toml");
        if !plan_toml.is_file() {
            continue;
        }
        let plan_name = path.file_name().unwrap_or_default();
        let target_plan_dir = target_plans_dir.join(plan_name);
        if target_plan_dir.exists() {
            info!("Plan {} already exists in target, skipping", plan_name.to_string_lossy());
            continue;
        }
        copy_dir_all(&path, &target_plan_dir)
            .with_context(|| format!("copy plan {} to target", plan_name.to_string_lossy()))?;
    }
    Ok(())
}

/// Copy group files into the target root.
fn copy_groups_to_target(source_groups: &[PathBuf], target_groups_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(target_groups_dir)?;
    for source in source_groups {
        let file_name = source.file_name().unwrap_or_default();
        let target = target_groups_dir.join(file_name);
        if target.exists() {
            info!("Group {} already exists in target, skipping", file_name.to_string_lossy());
            continue;
        }
        std::fs::copy(source, target)
            .with_context(|| format!("copy group {} to target", file_name.to_string_lossy()))?;
    }
    Ok(())
}

/// Recursively copy a directory tree.
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let dest = dst.join(&file_name);
        if path.is_dir() {
            copy_dir_all(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

/// Write a minimal wright.toml into the target root so the installed
/// system can operate independently.
fn write_target_wright_toml(root_dir: &Path, config: &GlobalConfig) -> Result<()> {
    let target_config = root_dir.join("etc/wright/wright.toml");
    std::fs::create_dir_all(target_config.parent().unwrap())?;

    let content = format!(
        r#"[general]
arch = "{}"
plans_dir = "/var/lib/wright/plans"
parts_dir = "/var/lib/wright/parts"
source_dir = "/var/lib/wright/sources"
db_path = "/var/lib/wright/wright.db"
logs_dir = "/var/log/wright"
executors_dir = "/etc/wright/executors"

[build]
build_dir = "/var/tmp/wright/workshop"
default_isolation = "{}"
"#,
        config.general.arch,
        config.build.default_isolation,
    );
    std::fs::write(&target_config, content)
        .with_context(|| format!("write {}", target_config.display()))?;
    Ok(())
}

async fn apply_group_config(
    root_dir: &Path,
    cfg: &group::GroupConfig,
) -> Result<()> {
    use crate::workflow::steps::{ApplyConfigInputs, ApplyConfigStep};
    use crate::workflow::WorkflowBuilder;

    let inputs = ApplyConfigInputs {
        root_dir: root_dir.to_path_buf(),
        hostname: cfg.hostname.clone(),
        timezone: cfg.timezone.clone(),
        locale: cfg.locale.clone(),
        services: {
            let mut s = cfg.services.clone();
            s.sort();
            s.dedup();
            s
        },
    };

    let mut wfb = WorkflowBuilder::new("apply_config", &inputs)
        .map_err(|e| anyhow::anyhow!("workflow builder: {}", e))?;
    wfb.add(ApplyConfigStep::new(inputs, Vec::new()))
        .map_err(|e| anyhow::anyhow!("apply config step: {}", e))?;
    let spec = wfb.build();

    drive_command(
        spec,
        DriveOptions {
            config: &GlobalConfig::default(),
            db_path: &root_dir.join("var/lib/wright/wright.db"),
            fresh: false,
            quiet: true,
        },
    )
    .await
    .map(|_| ())
}

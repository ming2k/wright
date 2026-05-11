use std::path::Path;

use tracing::info;

use crate::builder::logging;
use crate::builder::mvp::{cycle_candidates_for, find_cycles, format_cycle_path, pick_candidate};
use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::fhs;
use crate::part::lint::{lint_runtime_deps, LintReport, SonameIndex};
use crate::plan::manifest::{OutputConfig, PlanManifest};

pub fn lint_dependency_graph(graph: &crate::builder::mvp::PlanGraph) -> Result<()> {
    let cycles = find_cycles(&graph.deps_map);

    println!("Dependency Analysis Report");
    println!(
        "Status: {}",
        if cycles.is_empty() {
            "acyclic"
        } else {
            "cyclic"
        }
    );

    if cycles.is_empty() {
        return Ok(());
    }

    println!();
    println!("Cycles ({}):", cycles.len());
    for (idx, cycle) in cycles.iter().enumerate() {
        println!("{}: {}", idx + 1, format_cycle_path(cycle, &graph.deps_map));
    }

    println!();
    println!("MVP Candidates (deterministic pick = fewest excluded edges, then name):");
    println!("Cycle | Candidate | Excludes | Selected");
    println!("----- | --------- | -------- | --------");
    for (idx, cycle) in cycles.iter().enumerate() {
        let candidates = cycle_candidates_for(cycle, graph);
        if candidates.is_empty() {
            println!("{} | - | - | no candidates", idx + 1);
            continue;
        }
        let chosen = pick_candidate(candidates.clone());
        for cand in candidates {
            let selected = match &chosen {
                Some(c) if c.part == cand.part && c.excluded == cand.excluded => "yes",
                _ => "no",
            };
            println!(
                "{} | {} | {} | {}",
                idx + 1,
                cand.part,
                cand.excluded.join(", "),
                selected
            );
        }
    }

    Ok(())
}

/// Package the staging directories for a plan into `.wright.tar.zst` archives.
pub async fn package_outputs(
    manifest: &PlanManifest,
    config: &GlobalConfig,
    result: &crate::builder::BuildResult,
    print_parts: bool,
) -> Result<()> {
    tokio::fs::create_dir_all(&config.general.parts_dir)
        .await
        .map_err(WrightError::IoError)?;
    let output_dir = config.general.parts_dir.clone();

    // SONAME index is built once per package run, scoped to the plan's
    // declared link_deps closure. Per ADR-0017 this is a lint input only,
    // never written back into PARTINFO. When link_deps is empty the
    // index is also empty — a part that links nothing should not have
    // any DT_NEEDED edges to begin with.
    //
    // Advisory warnings (stale/unmapped) are intentionally NOT emitted
    // during packaging. In batch builds the dependency closure is usually
    // incomplete, making them unactionable noise. Use `wright doctor` after
    // full installation to surface them globally.
    let soname_index = SonameIndex::scan_for_link_deps(&output_dir, &manifest.link_deps)
        .unwrap_or_else(|e| {
            tracing::warn!(
                "elf-lint: failed to build SONAME index ({}); error-only lint will proceed",
                e
            );
            SonameIndex::default()
        });

    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => {
            for (sub_name, sub_part) in parts {
                let part_dir = if sub_part.include.is_none() {
                    &result.output_dir
                } else {
                    result.split_part_dirs.get(sub_name).ok_or_else(|| {
                        WrightError::BuildError(format!("missing output dir for '{}'", sub_name))
                    })?
                };
                if !manifest.options.skip_fhs_check {
                    fhs::validate(part_dir, sub_name)?;
                }
                let sub_manifest = sub_part.to_manifest(sub_name, manifest);
                maybe_run_elf_lint(part_dir, &sub_manifest, &soname_index)?;
                let sub_part_path =
                    archive::create_part(part_dir, &sub_manifest, &output_dir, Some(manifest))?;
                info!("{}", logging::plan_packed(sub_name, &sub_part_path));
                if print_parts {
                    println!("{}", sub_part_path.display());
                }
            }
        }
        _ => {
            if !manifest.options.skip_fhs_check {
                fhs::validate(&result.output_dir, &manifest.metadata.name)?;
            }
            maybe_run_elf_lint(&result.output_dir, manifest, &soname_index)?;
            let part_path = archive::create_part(&result.output_dir, manifest, &output_dir, None)?;
            info!(
                "{}",
                logging::plan_packed(&manifest.metadata.name, &part_path)
            );
            if print_parts {
                println!("{}", part_path.display());
            }
        }
    }

    Ok(())
}

/// Conditionally run the ADR-0017 ELF lint against a staged output.
///
/// Skipped when the plan opts out via `options.skip_elf_lint = true`,
/// or when the plan is marked `static = true` with no `link_deps` — a
/// common pattern for Go / Rust binaries that have no dynamic edges to
/// validate.
///
/// Only **errors** (forgotten runtime_deps) fail the package step.
/// Advisory items (stale, unmapped) are intentionally not reported here;
/// they are surfaced globally by `wright doctor` after installation when
/// the dependency closure is complete.
fn maybe_run_elf_lint(
    part_dir: &Path,
    manifest: &PlanManifest,
    index: &SonameIndex,
) -> Result<()> {
    if manifest.options.skip_elf_lint {
        tracing::debug!(
            "elf-lint: skipping {} (skip_elf_lint = true)",
            manifest.metadata.name
        );
        return Ok(());
    }

    if manifest.options.static_ && manifest.link_deps.is_empty() {
        tracing::debug!(
            "elf-lint: skipping {} (static build with no link_deps)",
            manifest.metadata.name
        );
        return Ok(());
    }

    let report = lint_runtime_deps(
        part_dir,
        &manifest.runtime_deps,
        &manifest.metadata.name,
        index,
    )?;
    if report.has_errors() {
        return Err(elf_lint_error(&manifest.metadata.name, &report));
    }
    Ok(())
}

fn elf_lint_error(part_name: &str, report: &LintReport) -> WrightError {
    let mut msg = format!(
        "elf-lint[{}]: forgotten runtime_deps detected — the binary needs \
         libraries that are not declared:\n",
        part_name
    );
    for f in &report.forgotten {
        msg.push_str(&format!(
            "  - {} (provided by '{}', linked by {})\n",
            f.soname,
            f.providing_output,
            f.seen_in.display()
        ));
    }
    msg.push_str(
        "Add the listed providers to runtime_deps in plan source (PARTINFO \
         and the registry remain plan-driven; this lint never auto-injects).",
    );
    WrightError::BuildError(msg)
}

/// Package a plan from its existing staging directories.
///
/// When `force` is true, or when `outputs/` is missing / stale, the staging
/// directory is re-sliced according to the current plan manifest before
/// packaging.  This lets users tweak `[[output]]` patterns and re-package
/// without running a full rebuild.
pub async fn package_manifest(
    manifest: &PlanManifest,
    config: &GlobalConfig,
    print_parts: bool,
    force: bool,
) -> Result<()> {
    let builder = crate::builder::Builder::new(config.clone());
    let build_root = builder.build_root(manifest)?;
    let output_dir = build_root.join("outputs").join("default");

    let need_slice = force
        || !output_dir.exists()
        || manifest.outputs.as_ref().is_some_and(|cfg| match cfg {
            OutputConfig::Multi(parts) => parts.iter().any(|(sub_name, sub_part)| {
                sub_part.include.is_some() && !build_root.join("outputs").join(sub_name).exists()
            }),
        });

    let result = if need_slice {
        builder.slice_outputs(manifest, &build_root).await?
    } else {
        let mut split_part_dirs = std::collections::HashMap::new();
        if let Some(OutputConfig::Multi(ref parts)) = manifest.outputs {
            for (sub_name, sub_part) in parts {
                if sub_part.include.is_none() {
                    continue;
                }
                split_part_dirs.insert(sub_name.clone(), build_root.join("outputs").join(sub_name));
            }
        }
        crate::builder::BuildResult {
            output_dir,
            work_dir: build_root.join("work"),
            logs_dir: build_root.join("logs"),
            split_part_dirs,
        }
    };

    package_outputs(manifest, config, &result, print_parts).await
}

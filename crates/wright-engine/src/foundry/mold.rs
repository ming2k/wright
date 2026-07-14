use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use crate::error::{Result, WrightError};
use crate::plan::manifest::{OutputConfig, PlanManifest};

/// Result of slicing the staging directory into outputs.
pub struct MoldResult {
    pub default_dir: PathBuf,
    pub split_dirs: HashMap<String, PathBuf>,
}

/// Slices the staging directory into output directories based on the plan's
/// `[[output]]` configuration.
///
/// The foundry metaphor: **Mold** is the workshop where the forged artifact is
/// poured into molds — each output is a distinct casting from the same staging
/// metal.
pub struct Mold;

impl Mold {
    pub async fn slice(manifest: &PlanManifest, build_root: &Path) -> Result<MoldResult> {
        let staging_dir = build_root.join("staging");
        let outputs_dir = build_root.join("outputs");
        let default_output_dir = outputs_dir.join("default");

        if !staging_dir.exists() {
            return Err(WrightError::ForgeError(format!(
                "staging directory does not exist: {}. Run `wright build {}` first.",
                staging_dir.display(),
                manifest.metadata.name
            )));
        }

        ensure_clean_dir(&outputs_dir).await?;
        tokio::fs::create_dir_all(&default_output_dir)
            .await
            .map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to create default output directory {}: {e}",
                    default_output_dir.display()
                ))
            })?;

        let mut split_dirs = HashMap::new();

        if let Some(OutputConfig::Multi(ref parts)) = manifest.outputs {
            let has_catchall = parts.iter().any(|(_, sub_part)| sub_part.include.is_none());
            let mut sub_rules: Vec<(
                &str,
                PathBuf,
                Vec<globset::GlobMatcher>,
                Vec<globset::GlobMatcher>,
            )> = Vec::new();

            for (sub_name, sub_part) in parts {
                let incs = match &sub_part.include {
                    Some(v) => v,
                    None => continue,
                };
                let sub_output_dir = outputs_dir.join(sub_name);
                tokio::fs::create_dir_all(&sub_output_dir)
                    .await
                    .map_err(|e| {
                        WrightError::ForgeError(format!(
                            "failed to create output directory {}: {e}",
                            sub_output_dir.display()
                        ))
                    })?;
                let includes = incs
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid include glob '{pat}' for {sub_name}: {e}"
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                let excludes = sub_part
                    .exclude
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid exclude glob '{pat}' for {sub_name}: {e}"
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                split_dirs.insert(sub_name.clone(), sub_output_dir.clone());
                sub_rules.push((sub_name.as_str(), sub_output_dir, includes, excludes));
            }

            let mut discard_rules: Vec<(
                &str,
                Vec<globset::GlobMatcher>,
                Vec<globset::GlobMatcher>,
            )> = Vec::new();
            for discard in &manifest.discard {
                let includes = discard
                    .include
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid discard include glob '{pat}': {e}"
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                let excludes = discard
                    .exclude
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid discard exclude glob '{pat}': {e}"
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                discard_rules.push((discard.reason.as_str(), includes, excludes));
            }

            debug!(
                "Splitting staging dir into {} outputs and {} discard rules",
                sub_rules.len(),
                discard_rules.len()
            );

            let mut all_entries = Vec::new();
            let mut symlink_entries = Vec::new();
            let mut dirs_to_visit = vec![staging_dir.clone()];
            while let Some(dir) = dirs_to_visit.pop() {
                if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        let file_type = match tokio::fs::symlink_metadata(&path).await {
                            Ok(m) => m.file_type(),
                            Err(_) => continue,
                        };
                        if file_type.is_symlink() {
                            symlink_entries.push(path);
                        } else if file_type.is_dir() {
                            dirs_to_visit.push(path);
                        } else {
                            all_entries.push(path);
                        }
                    }
                }
            }
            all_entries.sort();
            symlink_entries.sort();

            let mut link_actions = Vec::new();
            let mut symlink_actions = Vec::new();
            let mut unmatched = Vec::new();

            let find_matches = |rel_str: &str| -> Vec<(&str, &PathBuf)> {
                let mut matches = Vec::new();
                for (sub_name, sub_dir, includes, excludes) in &sub_rules {
                    let mut matched = includes.iter().any(|m| m.is_match(rel_str));
                    if matched
                        && !excludes.is_empty()
                        && excludes.iter().any(|m| m.is_match(rel_str))
                    {
                        matched = false;
                    }
                    if matched {
                        matches.push((*sub_name, sub_dir));
                    }
                }
                matches
            };

            for file_path in &all_entries {
                if let Ok(rel_path) = file_path.strip_prefix(&staging_dir) {
                    let rel_str = format!("/{}", rel_path.display());
                    let mut dest_path = None;
                    let matches = find_matches(&rel_str);
                    match matches.len() {
                        0 => {
                            let discarded =
                                discard_rules.iter().any(|(_reason, includes, excludes)| {
                                    let matched = includes.iter().any(|m| m.is_match(&rel_str));
                                    matched
                                        && (excludes.is_empty()
                                            || !excludes.iter().any(|m| m.is_match(&rel_str)))
                                });
                            if !discarded {
                                if has_catchall {
                                    dest_path = Some(default_output_dir.join(rel_path));
                                } else {
                                    unmatched.push(rel_str);
                                }
                            }
                        }
                        1 => {
                            dest_path = Some(matches[0].1.join(rel_path));
                        }
                        _ => {
                            let names: Vec<_> = matches.iter().map(|(n, _)| *n).collect();
                            return Err(WrightError::ForgeError(format!(
                                "ambiguous: file '{rel_str}' is matched by multiple outputs: {}. \
                                 Adjust include/exclude patterns so that each file is claimed by at most one output.",
                                names.join(", ")
                            )));
                        }
                    }
                    if let Some(dest_path) = dest_path {
                        link_actions.push((file_path.clone(), dest_path));
                    }
                }
            }

            for symlink_path in &symlink_entries {
                if let Ok(rel_path) = symlink_path.strip_prefix(&staging_dir) {
                    let rel_str = format!("/{}", rel_path.display());
                    let mut dest_path = None;
                    let matches = find_matches(&rel_str);
                    match matches.len() {
                        0 => {
                            let discarded =
                                discard_rules.iter().any(|(_reason, includes, excludes)| {
                                    let matched = includes.iter().any(|m| m.is_match(&rel_str));
                                    matched
                                        && (excludes.is_empty()
                                            || !excludes.iter().any(|m| m.is_match(&rel_str)))
                                });
                            if !discarded {
                                if has_catchall {
                                    dest_path = Some(default_output_dir.join(rel_path));
                                } else {
                                    unmatched.push(rel_str);
                                }
                            }
                        }
                        1 => {
                            dest_path = Some(matches[0].1.join(rel_path));
                        }
                        _ => {
                            let names: Vec<_> = matches.iter().map(|(n, _)| *n).collect();
                            return Err(WrightError::ForgeError(format!(
                                "ambiguous: symlink '{rel_str}' is matched by multiple outputs: {}. \
                                 Adjust include/exclude patterns so that each file is claimed by at most one output.",
                                names.join(", ")
                            )));
                        }
                    }
                    if let Some(dest_path) = dest_path {
                        let target = tokio::fs::read_link(symlink_path).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to read symlink {}: {e}",
                                symlink_path.display()
                            ))
                        })?;
                        symlink_actions.push((dest_path, target));
                    }
                }
            }

            if !unmatched.is_empty() {
                let shown = unmatched
                    .iter()
                    .take(50)
                    .map(|p| format!("  - {p}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let omitted = unmatched.len().saturating_sub(50);
                let suffix = if omitted > 0 {
                    format!("\n  ... and {omitted} more")
                } else {
                    String::new()
                };

                let logs_dir = build_root.join("logs");
                let _ = tokio::fs::create_dir_all(&logs_dir).await;
                let log_path = logs_dir.join("slice-errors.log");
                if let Ok(mut f) = std::fs::File::create(&log_path) {
                    use std::io::Write;
                    let _ = writeln!(f, "plan = {}", manifest.metadata.name);
                    let _ = writeln!(f, "staging_dir = {}", staging_dir.display());
                    let _ = writeln!(f, "unmatched_count = {}", unmatched.len());
                    let _ = writeln!(f);
                    for p in &unmatched {
                        let _ = writeln!(f, "{p}");
                    }
                    info!("Full unmatched file list written to {}", log_path.display());
                }

                return Err(WrightError::ForgeError(format!(
                    "{} staging files are not claimed by any [[output]] or [[discard]] rule:\n{}{}\nAdd an [[output]] include pattern, add an explicit [[discard]] rule, or add a catch-all [[output]] with no include.\nFull list: {}",
                    unmatched.len(),
                    shown,
                    suffix,
                    log_path.display()
                )));
            }

            for (file_path, dest_path) in link_actions {
                if let Some(parent) = dest_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(e) = link_or_copy(&file_path, &dest_path).await {
                    return Err(WrightError::ForgeError(format!(
                        "failed to link {} to {}: {e}",
                        file_path.display(),
                        dest_path.display()
                    )));
                }
            }

            for (dest_path, target) in symlink_actions {
                if let Some(parent) = dest_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                tokio::fs::symlink(&target, &dest_path).await.map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to create symlink {} -> {}: {e}",
                        dest_path.display(),
                        target.display()
                    ))
                })?;
            }
        } else {
            hard_link_all(&staging_dir, &default_output_dir).await?;
        }

        Ok(MoldResult {
            default_dir: default_output_dir,
            split_dirs,
        })
    }

    /// Standalone: regenerate outputs from an existing staging dir.
    pub async fn reslice(manifest: &PlanManifest, build_root: &Path) -> Result<MoldResult> {
        Self::slice(manifest, build_root).await
    }
}

async fn ensure_clean_dir(dir: &Path) -> Result<()> {
    if tokio::fs::metadata(dir).await.is_ok() {
        tokio::fs::remove_dir_all(dir).await.map_err(|e| {
            WrightError::ForgeError(format!("failed to clean directory {}: {e}", dir.display()))
        })?;
    }
    tokio::fs::create_dir_all(dir).await.map_err(|e| {
        WrightError::ForgeError(format!("failed to create directory {}: {e}", dir.display()))
    })
}

async fn hard_link_all(src_dir: &Path, dest_dir: &Path) -> Result<()> {
    let mut dirs_to_visit = vec![src_dir.to_path_buf()];
    while let Some(dir) = dirs_to_visit.pop() {
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let file_type = match tokio::fs::symlink_metadata(&path).await {
                    Ok(m) => Some(m.file_type()),
                    Err(_) => None,
                };

                let Ok(rel_path) = path.strip_prefix(src_dir) else {
                    continue;
                };
                let dest_path = dest_dir.join(rel_path);
                if let Some(parent) = dest_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }

                match file_type {
                    Some(ft) if ft.is_symlink() => {
                        let target = tokio::fs::read_link(&path).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to read symlink {}: {e}",
                                path.display()
                            ))
                        })?;
                        tokio::fs::symlink(&target, &dest_path).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to create symlink {} -> {}: {e}",
                                dest_path.display(),
                                target.display()
                            ))
                        })?;
                    }
                    Some(ft) if ft.is_dir() => {
                        dirs_to_visit.push(path);
                    }
                    _ => {
                        if let Err(e) = link_or_copy(&path, &dest_path).await {
                            return Err(WrightError::ForgeError(format!(
                                "failed to link {} to {}: {e}",
                                path.display(),
                                dest_path.display()
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

async fn link_or_copy(src: &Path, dest: &Path) -> std::io::Result<()> {
    match tokio::fs::hard_link(src, dest).await {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            tokio::fs::copy(src, dest).await.map(|_| ())
        }
        Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
            tokio::fs::copy(src, dest).await.map(|_| ())
        }
        Err(e) => Err(e),
    }
}

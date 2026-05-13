//! SONAME index and runtime-deps validation.
//!
//! `SonameIndex` maps SONAMEs to the `wright` part outputs that provide
//! them, built by scanning `.wright.tar.zst` archives under `parts_dir`.
//! It is used by `wright doctor` to verify the global dependency closure.
//!
//! `lint_runtime_deps` compares an output's declared `runtime_deps` against
//! the empirical `DT_NEEDED` set of its ELF binaries. Currently unused in
//! the plan execution lifecycle; retained for potential use by `doctor`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use walkdir::WalkDir;

use crate::error::Result;
use crate::part::archive::{ArchiveMeta, read_archive_meta};
use crate::part::elf;
use crate::part::version;

/// In-process cache keyed by (output_dir, mtime) → ElfMetadata.
///
/// During batch builds the same output directory is often packaged
/// repeatedly (e.g. re-slice without rebuild). This avoids re-walking
/// the tree and re-parsing every ELF header.
fn elf_lint_cache() -> &'static Mutex<HashMap<(PathBuf, SystemTime), ElfMetadata>> {
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, SystemTime), ElfMetadata>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Map SONAME → (output_name, plan_name) built by scanning archives.
///
/// Output names are globally unique (UNIQUE constraint on `parts.name`)
/// so a single string suffices to identify the providing part. The plan
/// name is kept alongside so bare-plan declarations can be expanded.
#[derive(Debug, Default)]
pub struct SonameIndex {
    soname_to_output: HashMap<String, String>,
    plan_outputs: HashMap<String, HashSet<String>>,
}

impl SonameIndex {
    /// Walk every `.wright.tar.zst` archive under `parts_dir` and build
    /// the SONAME index. Archives that fail to parse are skipped with a
    /// warning; one bad archive must not break a whole seal run.
    ///
    /// Prefer `scan_for_link_deps` when the caller has a known link-deps
    /// list — restricting the index to that closure avoids false-positive
    /// forgotten errors when an unrelated archive happens to ship the
    /// same SONAME.
    pub fn scan_parts_dir(parts_dir: &Path) -> Result<Self> {
        Self::scan_filtered(parts_dir, |_meta| true)
    }

    /// Restricted scan: only include archives whose plan name appears in
    /// `link_deps`, or whose output (part) name appears verbatim. Bare
    /// `plan` and `plan:output` forms are both honored; unparseable
    /// entries are ignored.
    ///
    /// If `link_deps` is empty, returns an empty index (no link → no
    /// runtime SONAMEs expected).
    pub fn scan_for_link_deps(parts_dir: &Path, link_deps: &[String]) -> Result<Self> {
        if link_deps.is_empty() {
            return Ok(Self::default());
        }
        let mut allowed_plans = HashSet::new();
        let mut allowed_outputs = HashSet::new();
        for dep in link_deps {
            let dep = dep.trim();
            if dep.is_empty() {
                continue;
            }
            let (dep_ref, _) = match version::parse_dependency(dep) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let (plan, output) = version::parse_dep_ref(&dep_ref).to_plan_output();
            if !output.is_empty() {
                allowed_outputs.insert(output);
            } else {
                allowed_plans.insert(plan);
            }
        }
        Self::scan_filtered(parts_dir, |meta| {
            allowed_plans.contains(&meta.partinfo.plan.name)
                || allowed_outputs.contains(&meta.partinfo.name)
        })
    }

    fn scan_filtered<F>(parts_dir: &Path, mut keep: F) -> Result<Self>
    where
        F: FnMut(&ArchiveMeta) -> bool,
    {
        let mut idx = Self::default();
        if !parts_dir.exists() {
            return Ok(idx);
        }

        for entry in std::fs::read_dir(parts_dir)
            .map_err(|e| {
                crate::error::WrightError::PartError(format!("read {}: {}", parts_dir.display(), e))
            })?
            .flatten()
        {
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|f| f.to_str())
                .map(|n| n.ends_with(".wright.tar.zst"))
                .unwrap_or(false)
            {
                continue;
            }

            match read_archive_meta(&path) {
                Ok(meta) => {
                    if keep(&meta) {
                        idx.absorb(meta);
                    }
                }
                Err(e) => tracing::warn!(
                    "elf-lint: skipping unreadable archive {}: {}",
                    path.display(),
                    e
                ),
            }
        }
        Ok(idx)
    }

    fn absorb(&mut self, meta: ArchiveMeta) {
        let output_name = meta.partinfo.name.clone();
        let plan_name = meta.partinfo.plan.name.clone();
        self.plan_outputs
            .entry(plan_name)
            .or_default()
            .insert(output_name.clone());

        for path in &meta.files {
            if let Some(soname) = soname_from_filename(path) {
                self.soname_to_output
                    .entry(soname)
                    .or_insert_with(|| output_name.clone());
            }
        }
    }

    /// Resolve a SONAME to the output that provides it.
    pub fn output_for_soname(&self, soname: &str) -> Option<&str> {
        self.soname_to_output.get(soname).map(String::as_str)
    }

    /// All output names produced by the named plan (empty set if unknown).
    pub fn outputs_of(&self, plan: &str) -> Option<&HashSet<String>> {
        self.plan_outputs.get(plan)
    }

    /// Returns true when the index contains no entries.
    pub fn is_empty(&self) -> bool {
        self.soname_to_output.is_empty() && self.plan_outputs.is_empty()
    }
}

/// Result of linting one output against its declared `runtime_deps`.
#[derive(Debug, Default)]
pub struct LintReport {
    /// SONAMEs the binary needs whose providing output is not declared
    /// in `runtime_deps`. Direction-C: error.
    pub forgotten: Vec<ForgottenDep>,
    /// Declared deps with no DT_NEEDED edge to them. Direction-C: warning.
    pub stale: Vec<String>,
    /// SONAMEs not provided by any indexed archive. Direction-C: warning.
    pub unmapped: Vec<UnmappedSoname>,
}

#[derive(Debug)]
pub struct ForgottenDep {
    pub soname: String,
    pub providing_output: String,
    pub seen_in: PathBuf,
}

#[derive(Debug)]
pub struct UnmappedSoname {
    pub soname: String,
    pub seen_in: PathBuf,
}

impl LintReport {
    pub fn has_errors(&self) -> bool {
        !self.forgotten.is_empty()
    }
}

/// Raw ELF metadata extracted from a single walk of the output directory.
#[derive(Debug, Clone)]
struct ElfMetadata {
    needed: HashSet<String>,
    ownership: HashMap<String, PathBuf>,
    self_sonames: HashSet<String>,
}

/// Lint one output's staging tree against its declared `runtime_deps`.
///
/// `output_dir` is the staged tree about to be packaged. `declared` is
/// the verbatim `runtime_deps` list from the plan source. `self_part_name`
/// is the output name being packaged — its own SONAMEs are filtered out
/// of `DT_NEEDED` so a part that links its own libraries does not appear
/// to depend on itself.
///
/// This function caches the ELF scan keyed by `(output_dir, mtime)` so
/// repeated sealing of the same unchanged tree is effectively free.
pub fn lint_runtime_deps(
    output_dir: &Path,
    declared: &[String],
    self_part_name: &str,
    index: &SonameIndex,
) -> Result<LintReport> {
    let mtime = dir_mtime(output_dir)?;
    let meta = {
        let mut cache = elf_lint_cache().lock().map_err(|e| {
            crate::error::WrightError::PartError(format!("elf-lint cache poison: {}", e))
        })?;
        if let Some(cached) = cache.get(&(output_dir.to_path_buf(), mtime)) {
            cached.clone()
        } else {
            let fresh = collect_elf_metadata(output_dir)?;
            cache.insert((output_dir.to_path_buf(), mtime), fresh.clone());
            fresh
        }
    };
    evaluate_lint_report(declared, self_part_name, index, &meta)
}

fn evaluate_lint_report(
    declared: &[String],
    self_part_name: &str,
    index: &SonameIndex,
    meta: &ElfMetadata,
) -> Result<LintReport> {
    let needed_external: BTreeSet<&String> = meta.needed.difference(&meta.self_sonames).collect();

    let declared_targets = expand_declared_targets(declared, index, self_part_name);

    let mut report = LintReport::default();
    let mut matched_targets: HashSet<String> = HashSet::new();

    for soname in &needed_external {
        match index.output_for_soname(soname) {
            Some(output) => {
                if declared_targets.contains(output) {
                    matched_targets.insert(output.to_string());
                } else {
                    report.forgotten.push(ForgottenDep {
                        soname: (*soname).clone(),
                        providing_output: output.to_string(),
                        seen_in: meta.ownership.get(*soname).cloned().unwrap_or_default(),
                    });
                }
            }
            None => report.unmapped.push(UnmappedSoname {
                soname: (*soname).clone(),
                seen_in: meta.ownership.get(*soname).cloned().unwrap_or_default(),
            }),
        }
    }

    // Stale: declared but no soname routed to its target output set.
    for dep in declared {
        let dep = dep.trim();
        if dep.is_empty() {
            continue;
        }
        let targets = targets_for_dep(dep, index);
        if targets.is_disjoint(&matched_targets) && !targets.is_empty() {
            report.stale.push(dep.to_string());
        } else if targets.is_empty() {
            // Declared dep does not resolve to anything wright knows about.
            // Treated as stale rather than unmapped because it is a plan
            // source authoring error, not a build sysroot mystery.
            report.stale.push(dep.to_string());
        }
    }

    Ok(report)
}

/// Walk the output directory once and collect DT_NEEDED, DT_SONAME, and
/// basename heuristics for every ELF file.
fn collect_elf_metadata(output_dir: &Path) -> Result<ElfMetadata> {
    let mut needed = HashSet::new();
    let mut ownership: HashMap<String, PathBuf> = HashMap::new();
    let mut self_sonames = HashSet::new();

    for entry in WalkDir::new(output_dir).into_iter().flatten() {
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }

        // DT_NEEDED
        if let Ok(Some(libs)) = elf::read_dt_needed(path) {
            for lib in libs {
                ownership
                    .entry(lib.clone())
                    .or_insert_with(|| path.to_path_buf());
                needed.insert(lib);
            }
        }

        // DT_SONAME (self-provided libraries)
        if let Ok(Some(soname)) = elf::read_dt_soname(path) {
            self_sonames.insert(soname);
        }
        // Also include the basename for .so files even when DT_SONAME is
        // absent — many builds produce libfoo.so.N without the tag, but
        // sibling binaries link via that filename anyway.
        if let Some(base) = path
            .file_name()
            .and_then(|f| f.to_str())
            .filter(|n| n.contains(".so"))
        {
            self_sonames.insert(base.to_string());
        }
    }

    Ok(ElfMetadata {
        needed,
        ownership,
        self_sonames,
    })
}

/// Heuristic mtime for a directory: latest mtime of any direct child entry.
/// Using recursive mtime would be expensive, so we approximate with the
/// directory's own mtime plus direct children. This is good enough because
/// sealing always rewrites the output directory when it changes.
fn dir_mtime(dir: &Path) -> Result<SystemTime> {
    let mut latest = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && let Ok(t) = meta.modified()
                    && t > latest {
                        latest = t;
                    }
        }
    }
    Ok(latest)
}

/// Expand the user's declared deps into the concrete output-name set we
/// expect to see SONAMEs route to.
fn expand_declared_targets(
    declared: &[String],
    index: &SonameIndex,
    self_part_name: &str,
) -> HashSet<String> {
    let mut targets = HashSet::new();
    targets.insert(self_part_name.to_string());
    for dep in declared {
        for t in targets_for_dep(dep.trim(), index) {
            targets.insert(t);
        }
    }
    targets
}

fn targets_for_dep(dep: &str, index: &SonameIndex) -> HashSet<String> {
    let mut targets = HashSet::new();
    if dep.is_empty() {
        return targets;
    }
    let (dep_ref, _) = match version::parse_dependency(dep) {
        Ok(parsed) => parsed,
        Err(_) => return targets,
    };
    let (plan, output) = version::parse_dep_ref(&dep_ref).to_plan_output();
    if !output.is_empty() {
        targets.insert(output);
    } else if let Some(outs) = index.outputs_of(&plan) {
        for o in outs {
            targets.insert(o.clone());
        }
    } else {
        // Unknown plan — fall back to treating the whole token as an
        // output name, which is what bare names degrade to in practice
        // (single-output plans where plan_name == output_name).
        targets.insert(plan);
    }
    targets
}

/// Heuristic: extract a SONAME-shaped string from a file path's basename.
///
/// Known gap (ADR-0017 v0): the real `DT_SONAME` of a `.so` may differ
/// from its filename. Reading the actual tag would require per-archive
/// extraction (decompress, untar each `.so` to a temp file, parse). The
/// basename heuristic catches the conventional 99% case; the remaining
/// 1% surfaces as `unmapped` warnings, not silent miss. Promote to real
/// extraction if false-negative rate becomes an issue in practice.
fn soname_from_filename(path: &str) -> Option<String> {
    let base = path.rsplit('/').next()?;
    if !base.contains(".so") {
        return None;
    }
    Some(base.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soname_from_filename_picks_basename() {
        assert_eq!(
            soname_from_filename("/usr/lib/libssl.so.3"),
            Some("libssl.so.3".to_string())
        );
        assert_eq!(
            soname_from_filename("usr/lib/libfoo.so"),
            Some("libfoo.so".to_string())
        );
        assert_eq!(soname_from_filename("/usr/bin/bash"), None);
        assert_eq!(soname_from_filename("/etc/passwd"), None);
    }

    #[test]
    fn empty_index_gives_no_match() {
        let idx = SonameIndex::default();
        assert!(idx.output_for_soname("libssl.so.3").is_none());
    }

    #[test]
    fn report_is_clean_on_empty_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let idx = SonameIndex::default();
        let report = lint_runtime_deps(dir.path(), &[], "self", &idx).unwrap();
        assert!(!report.has_errors());
        assert!(report.forgotten.is_empty());
        assert!(report.stale.is_empty());
        assert!(report.unmapped.is_empty());
    }

    #[test]
    fn declared_dep_with_unknown_plan_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let idx = SonameIndex::default();
        let report =
            lint_runtime_deps(dir.path(), &["nonexistent".to_string()], "self", &idx).unwrap();
        // No DT_NEEDED, so nothing matched; declared dep can't route.
        assert_eq!(report.stale, vec!["nonexistent".to_string()]);
    }
}

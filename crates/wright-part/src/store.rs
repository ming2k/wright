use crate::archive;
use crate::error::{Result, WrightError};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use wright_model::version::Version;

#[inline]
pub fn sanitize_cache_filename(raw: &str) -> String {
    let name = raw.rsplit('/').next().unwrap_or(raw);
    let name = name.rsplit('\\').next().unwrap_or(name);
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character == '\0' {
                '_'
            } else {
                character
            }
        })
        .collect();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "download".to_string()
    } else {
        sanitized
    }
}

#[derive(Debug, Clone)]
pub struct LocalPartStore {
    pub search_dirs: Vec<PathBuf>,
}

pub struct ResolvedPart {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPartVersioned {
    pub name: String,
    /// Name of the plan that produced this part. Several plans may ship
    /// same-named outputs, so name alone does not identify an archive.
    pub plan_name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

impl ResolvedPartVersioned {
    pub fn version_cmp(&self, other: &Self) -> Ordering {
        if self.epoch != other.epoch {
            return self.epoch.cmp(&other.epoch);
        }
        let self_ver = Version::parse(&self.version).ok();
        let other_ver = Version::parse(&other.version).ok();
        match (self_ver, other_ver) {
            (Some(sv), Some(ov)) => {
                let ord = sv.cmp(&ov);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            _ => {
                let ord = self.version.cmp(&other.version);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
        self.release.cmp(&other.release)
    }
}

pub fn pick_latest(parts: &[ResolvedPartVersioned]) -> Option<&ResolvedPartVersioned> {
    parts.iter().max_by(|a, b| a.version_cmp(b))
}

pub fn pick_version<'a>(
    parts: &'a [ResolvedPartVersioned],
    version: &str,
) -> Option<&'a ResolvedPartVersioned> {
    parts
        .iter()
        .filter(|p| p.version == version)
        .max_by_key(|p| p.release)
}

impl Default for LocalPartStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalPartStore {
    pub fn new() -> Self {
        Self {
            search_dirs: Vec::new(),
        }
    }

    pub fn add_search_dir(&mut self, path: PathBuf) {
        self.search_dirs.push(path);
    }

    pub async fn resolve(&self, name: &str) -> Result<Option<ResolvedPart>> {
        self.resolve_local(name).await
    }

    async fn resolve_local(&self, name: &str) -> Result<Option<ResolvedPart>> {
        let all = self.resolve_all(name).await?;
        Ok(pick_latest(&all).map(|p| ResolvedPart {
            name: p.name.clone(),
            version: p.version.clone(),
            path: p.path.clone(),
            dependencies: p.dependencies.clone(),
        }))
    }

    /// Resolve a part that must come from a specific plan build.
    ///
    /// Unlike `resolve`, which matches any archive with the same part name
    /// and picks the highest version, this filters by the originating plan
    /// and the exact version/release/epoch. Use it when the caller knows
    /// which plan build the part must belong to — e.g. right after sealing
    /// that plan — so a foreign archive that happens to share the part name
    /// (possibly with a higher version) is never picked up by mistake.
    pub async fn resolve_from_plan(
        &self,
        name: &str,
        plan_name: &str,
        version: &str,
        release: u32,
        epoch: u32,
    ) -> Result<Option<ResolvedPartVersioned>> {
        let all = self.resolve_all_from_plan(name, plan_name).await?;
        Ok(all
            .into_iter()
            .find(|p| p.version == version && p.release == release && p.epoch == epoch))
    }

    /// All versions of a part produced by a specific plan — the plan-pinned
    /// counterpart of [`resolve_all`](Self::resolve_all). Use it when the
    /// caller knows the originating plan and only needs to pick a version,
    /// so archives from other plans never enter the candidate set.
    pub async fn resolve_all_from_plan(
        &self,
        name: &str,
        plan_name: &str,
    ) -> Result<Vec<ResolvedPartVersioned>> {
        let all = self.resolve_all(name).await?;
        Ok(all
            .into_iter()
            .filter(|p| p.plan_name == plan_name)
            .collect())
    }

    pub async fn resolve_all(&self, name: &str) -> Result<Vec<ResolvedPartVersioned>> {
        let search_dirs = self.search_dirs.clone();
        let name = name.to_string();

        tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            for dir in &search_dirs {
                if !dir.exists() {
                    continue;
                }
                // Recurse: current archives live in per-plan subdirectories
                // (`<dir>/<plan>/<file>`), older ones flat at the top level.
                // Identity always comes from .PARTINFO, never from the path.
                for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
                    let path = entry.path();
                    let fname = match path.file_name().and_then(|s| s.to_str()) {
                        Some(f) => f,
                        None => continue,
                    };
                    if !fname.ends_with(".wright.tar.zst") {
                        continue;
                    }
                    let partinfo = match archive::read_partinfo(path) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    if partinfo.name != name {
                        continue;
                    }
                    results.push(ResolvedPartVersioned {
                        name: partinfo.name,
                        plan_name: partinfo.plan.name,
                        version: partinfo.plan.version,
                        release: partinfo.plan.release,
                        epoch: partinfo.plan.epoch,
                        path: path.to_path_buf(),
                        dependencies: partinfo.runtime_deps,
                    });
                }
            }
            Ok(results)
        })
        .await
        .map_err(|e| WrightError::ForgeError(format!("part store task failed: {}", e)))?
    }

    pub fn read_part(&self, path: &Path) -> Result<ResolvedPart> {
        let partinfo = archive::read_partinfo(path)?;
        Ok(ResolvedPart {
            name: partinfo.name,
            version: partinfo.plan.version,
            path: path.to_path_buf(),
            dependencies: partinfo.runtime_deps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{PartHooks, PartSpec, PlanMetadata, Provenance, write_part};

    fn seal_part(parts_dir: &Path, staging_root: &Path, plan: &str, version: &str, release: u32) {
        // Legacy flat layout: archives directly in parts_dir.
        seal_part_at(
            parts_dir.to_path_buf(),
            staging_root,
            plan,
            version,
            release,
        );
    }

    fn seal_part_nested(
        parts_dir: &Path,
        staging_root: &Path,
        plan: &str,
        version: &str,
        release: u32,
    ) -> PathBuf {
        // Current layout: archives in the per-plan subdirectory.
        seal_part_at(parts_dir.join(plan), staging_root, plan, version, release)
    }

    fn seal_part_at(
        output_dir: PathBuf,
        staging_root: &Path,
        plan: &str,
        version: &str,
        release: u32,
    ) -> PathBuf {
        std::fs::create_dir_all(&output_dir).unwrap();
        let staging = staging_root.join(format!("staging-{plan}-{release}"));
        std::fs::create_dir_all(staging.join("usr/lib")).unwrap();
        std::fs::write(staging.join("usr/lib/payload"), plan).unwrap();
        let spec = PartSpec {
            archive_name: format!("prism-{version}-{release}-x86_64.wright.tar.zst"),
            name: "prism".to_string(),
            runtime_deps: Vec::new(),
            replaces: Vec::new(),
            conflicts: Vec::new(),
            backup_files: Vec::new(),
            plan: PlanMetadata {
                name: plan.to_string(),
                version: version.to_string(),
                release,
                epoch: 0,
                arch: "x86_64".to_string(),
            },
            provenance: Provenance {
                plan_checksum: None,
                source_checksums: Vec::new(),
                wright_version: env!("CARGO_PKG_VERSION").to_string(),
                isolation: "strict".to_string(),
            },
            plan_source: None,
            hooks: PartHooks::default(),
        };
        write_part(&staging, &spec, &output_dir).unwrap()
    }

    #[tokio::test]
    async fn resolve_from_plan_ignores_foreign_same_named_part() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();

        // Two unrelated plans ship a part named "prism": a stale foreign
        // build with a much higher version, and the current optics build.
        seal_part(&parts_dir, tmp.path(), "prism", "9.0.537", 1);
        seal_part(&parts_dir, tmp.path(), "optics", "0.0.14", 2);

        let mut store = LocalPartStore::new();
        store.add_search_dir(parts_dir);

        // Bare name resolution keeps its historical pick-latest behavior.
        let latest = store.resolve("prism").await.unwrap().unwrap();
        assert_eq!(latest.version, "9.0.537");

        // Plan-pinned resolution returns the part from the requested build.
        let pinned = store
            .resolve_from_plan("prism", "optics", "0.0.14", 2, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pinned.plan_name, "optics");
        assert_eq!(pinned.version, "0.0.14");
        assert_eq!(pinned.release, 2);

        // Wrong plan or wrong version must not match.
        assert!(
            store
                .resolve_from_plan("prism", "optics", "9.0.537", 1, 0)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .resolve_from_plan("prism", "nonexistent", "0.0.14", 2, 0)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn plan_subdirectories_keep_identical_versions_distinct() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();

        // Two plans ship an output named "prism" with the exact same
        // version/release/arch: the archives must coexist as
        // `<plan>/prism-1.0.0-1-x86_64.wright.tar.zst`.
        let optics_path = seal_part_nested(&parts_dir, tmp.path(), "optics", "1.0.0", 1);
        let prism_path = seal_part_nested(&parts_dir, tmp.path(), "prism", "1.0.0", 1);
        assert_ne!(optics_path, prism_path);
        assert_eq!(
            optics_path,
            parts_dir.join("optics/prism-1.0.0-1-x86_64.wright.tar.zst")
        );
        assert_eq!(
            prism_path,
            parts_dir.join("prism/prism-1.0.0-1-x86_64.wright.tar.zst")
        );

        let mut store = LocalPartStore::new();
        store.add_search_dir(parts_dir.clone());

        // The recursive scan finds both nested archives.
        let all = store.resolve_all("prism").await.unwrap();
        assert_eq!(all.len(), 2);

        // Plan-pinned resolution returns the archive from the requested plan.
        let pinned = store
            .resolve_from_plan("prism", "optics", "1.0.0", 1, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pinned.path, optics_path);
        assert_eq!(pinned.plan_name, "optics");

        let from_plan = store.resolve_all_from_plan("prism", "prism").await.unwrap();
        assert_eq!(from_plan.len(), 1);
        assert_eq!(from_plan[0].path, prism_path);
    }

    #[tokio::test]
    async fn recursive_scan_still_finds_legacy_flat_archives() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();

        // One archive sealed before the plan-subdirectory layout (flat),
        // one sealed after (nested): both must resolve.
        seal_part(&parts_dir, tmp.path(), "optics", "0.0.14", 2);
        seal_part_nested(&parts_dir, tmp.path(), "prism", "1.0.0", 1);

        let mut store = LocalPartStore::new();
        store.add_search_dir(parts_dir);

        let all = store.resolve_all("prism").await.unwrap();
        assert_eq!(all.len(), 2);
        let plans: Vec<&str> = {
            let mut plans: Vec<&str> = all.iter().map(|p| p.plan_name.as_str()).collect();
            plans.sort_unstable();
            plans
        };
        assert_eq!(plans, ["optics", "prism"]);
    }
}

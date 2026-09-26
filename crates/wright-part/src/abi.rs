//! Deterministic physical ABI extraction and compatibility diffing.
//!
//! Evaluates whether shared library (.so) outputs preserve backward
//! compatibility across plan updates, enabling safe rebuild inhibition
//! for downstream reverse link dependents.

use goblin::elf::Elf;
use goblin::elf::section_header::SHN_UNDEF;
use goblin::elf::sym::{STB_GLOBAL, STB_WEAK, STV_DEFAULT, STV_PROTECTED};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use walkdir::WalkDir;

use crate::error::{Result, WrightError};

fn is_elf_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == b"\x7fELF"
}

/// ABI representation for a single ELF shared object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElfAbi {
    /// SONAME if declared by the ELF dynamic section.
    pub soname: Option<String>,
    /// Libraries this object dynamically depends on (`DT_NEEDED`).
    pub needed: Vec<String>,
    /// Exported public symbols (functions, objects, ifuncs).
    pub exported_symbols: BTreeSet<String>,
    /// Deterministic content hash of this library's public ABI.
    pub abi_hash: String,
}

impl ElfAbi {
    /// Compute a canonical SHA-256 digest of this library's exported ABI.
    pub fn compute_hash(
        soname: Option<&str>,
        needed: &[String],
        exported_symbols: &BTreeSet<String>,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"soname:");
        if let Some(s) = soname {
            hasher.update(s.as_bytes());
        }
        hasher.update(b"\nneeded:");
        for dep in needed {
            hasher.update(dep.as_bytes());
            hasher.update(b",");
        }
        hasher.update(b"\nsymbols:\n");
        for sym in exported_symbols {
            hasher.update(sym.as_bytes());
            hasher.update(b"\n");
        }
        format!("{:x}", hasher.finalize())
    }
}

/// Aggregated ABI snapshot for all shared libraries in a part or staging tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PartAbi {
    /// Map of relative path (e.g. `usr/lib/libfoo.so.1`) to library ABI.
    pub libraries: BTreeMap<String, ElfAbi>,
    /// Deterministic overall ABI hash combining all libraries.
    pub abi_hash: String,
}

impl PartAbi {
    /// Recompute the overall ABI hash from the library map.
    pub fn recompute_overall_hash(&mut self) {
        let mut hasher = Sha256::new();
        for (rel_path, lib) in &self.libraries {
            hasher.update(rel_path.as_bytes());
            hasher.update(b"=");
            hasher.update(lib.abi_hash.as_bytes());
            hasher.update(b"\n");
        }
        self.abi_hash = format!("{:x}", hasher.finalize());
    }
}

/// The result of comparing an old PartAbi against a new PartAbi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AbiCompatibility {
    /// Exactly identical ABI hash across all libraries.
    Identical,
    /// Backward-compatible superset: all previously exported symbols and
    /// SONAMEs are preserved; new symbols may have been added.
    CompatibleSuperset { added_symbols: usize },
    /// Incompatible ABI change: downstream reverse dependents MUST be rebuilt.
    Incompatible(AbiBreakReason),
}

impl AbiCompatibility {
    /// Returns true if downstream dependents can safely skip recompilation.
    pub fn is_compatible(&self) -> bool {
        matches!(
            self,
            Self::Identical | Self::CompatibleSuperset { .. }
        )
    }
}

/// Precise failure reason when an ABI compatibility check fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AbiBreakReason {
    /// A previously exported shared library was completely removed.
    LibraryRemoved(String),
    /// The SONAME of a shared library changed.
    SonameChanged {
        library: String,
        old: Option<String>,
        new: Option<String>,
    },
    /// One or more exported symbols were removed or renamed.
    SymbolsRemoved {
        library: String,
        missing: Vec<String>,
    },
    /// The plan author explicitly bumped the `abi_epoch`.
    ExplicitEpochBump {
        old_epoch: u32,
        new_epoch: u32,
    },
    /// Plan explicitly declared `abi_stability = "inlined"` or similar policy.
    PolicyInlined,
    /// No shared libraries found in one or both artifacts.
    NoSharedLibraries,
}

impl std::fmt::Display for AbiBreakReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryRemoved(lib) => write!(f, "library removed: {lib}"),
            Self::SonameChanged { library, old, new } => {
                write!(
                    f,
                    "{library} SONAME changed from {} to {}",
                    old.as_deref().unwrap_or("<none>"),
                    new.as_deref().unwrap_or("<none>")
                )
            }
            Self::SymbolsRemoved { library, missing } => {
                if missing.len() <= 3 {
                    write!(
                        f,
                        "{library} symbols removed: {}",
                        missing.join(", ")
                    )
                } else {
                    write!(
                        f,
                        "{library} {} symbols removed (e.g. {}, {})",
                        missing.len(),
                        missing[0],
                        missing[1]
                    )
                }
            }
            Self::ExplicitEpochBump { old_epoch, new_epoch } => {
                write!(f, "explicit abi_epoch bump ({old_epoch} -> {new_epoch})")
            }
            Self::PolicyInlined => {
                write!(f, "plan declared inlined ABI stability (header/template heavy)")
            }
            Self::NoSharedLibraries => {
                write!(f, "no shared libraries present to prove ABI preservation")
            }
        }
    }
}

/// Extract the physical ABI from an ELF shared object file.
///
/// Returns `Ok(None)` if the file is not an ELF object, or has no dynamic
/// symbols/SONAME (e.g. static binary or non-library).
pub fn extract_elf_abi(path: &Path) -> Result<Option<ElfAbi>> {
    let bytes = std::fs::read(path)
        .map_err(|e| WrightError::context(format!("read {}", path.display()), e))?;

    if !is_elf_magic(&bytes) {
        return Ok(None);
    }

    let elf = match Elf::parse(&bytes) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let soname = elf.soname.map(|s| s.to_string());
    let needed: Vec<String> = elf.libraries.iter().map(|s| (*s).to_string()).collect();

    let mut exported_symbols = BTreeSet::new();

    for sym in &elf.dynsyms {
        // Must be defined in this ELF object (not an import)
        if sym.st_shndx == SHN_UNDEF as usize {
            continue;
        }

        // Must have global or weak binding
        let bind = sym.st_bind();
        if bind != STB_GLOBAL && bind != STB_WEAK {
            continue;
        }

        // Must have default or protected visibility (exported to linkers)
        let vis = sym.st_visibility();
        if vis != STV_DEFAULT && vis != STV_PROTECTED {
            continue;
        }

        if let Some(name) = elf.dynstrtab.get_at(sym.st_name) {
            if !name.is_empty() {
                exported_symbols.insert(name.to_string());
            }
        }
    }

    // Only consider it an ABI-bearing library if it exports symbols or has a SONAME
    if exported_symbols.is_empty() && soname.is_none() {
        return Ok(None);
    }

    let abi_hash = ElfAbi::compute_hash(soname.as_deref(), &needed, &exported_symbols);

    Ok(Some(ElfAbi {
        soname,
        needed,
        exported_symbols,
        abi_hash,
    }))
}

/// Scan a directory tree (such as a staging area or sysroot) and extract
/// all shared libraries into a PartAbi snapshot.
pub fn extract_part_abi(root: &Path) -> Result<PartAbi> {
    let mut libraries = BTreeMap::new();

    if !root.exists() {
        return Ok(PartAbi::default());
    }

    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry.map_err(|e| WrightError::context("walk part directory", e))?;
        // Only inspect regular files (skip symlinks like `libfoo.so -> libfoo.so.1`)
        if !entry.file_type().is_file() {
            continue;
        }

        let p = entry.path();
        if let Some(abi) = extract_elf_abi(p)? {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string();
            libraries.insert(rel, abi);
        }
    }

    let mut part_abi = PartAbi {
        libraries,
        abi_hash: String::new(),
    };
    part_abi.recompute_overall_hash();
    Ok(part_abi)
}

fn library_stem(path: &str) -> &str {
    let filename = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    if let Some(pos) = filename.find(".so") {
        &filename[..pos + 3]
    } else {
        filename
    }
}

/// Compare two PartAbi snapshots and determine compatibility.
pub fn diff_abi(old_abi: &PartAbi, new_abi: &PartAbi) -> AbiCompatibility {
    if old_abi.libraries.is_empty() && new_abi.libraries.is_empty() {
        return AbiCompatibility::Incompatible(AbiBreakReason::NoSharedLibraries);
    }

    if old_abi.abi_hash == new_abi.abi_hash {
        return AbiCompatibility::Identical;
    }

    let mut total_added_symbols = 0;

    for (old_path, old_lib) in &old_abi.libraries {
        // Match library:
        // 1. By exact relative path
        // 2. By SONAME if identical
        // 3. By shared library stem (e.g. `libtest.so.1` and `libtest.so.2` share `libtest.so`)
        let new_lib = new_abi
            .libraries
            .get(old_path)
            .or_else(|| {
                if let Some(ref soname) = old_lib.soname {
                    new_abi
                        .libraries
                        .values()
                        .find(|l| l.soname.as_deref() == Some(soname.as_str()))
                } else {
                    None
                }
            })
            .or_else(|| {
                let old_stem = library_stem(old_path);
                new_abi.libraries.iter().find_map(|(np, nl)| {
                    if library_stem(np) == old_stem {
                        Some(nl)
                    } else {
                        None
                    }
                })
            });

        let Some(new_lib) = new_lib else {
            return AbiCompatibility::Incompatible(AbiBreakReason::LibraryRemoved(old_path.clone()));
        };

        // 1. SONAME check
        if old_lib.soname != new_lib.soname {
            return AbiCompatibility::Incompatible(AbiBreakReason::SonameChanged {
                library: old_path.clone(),
                old: old_lib.soname.clone(),
                new: new_lib.soname.clone(),
            });
        }

        // 2. Exported symbols check
        let missing: Vec<String> = old_lib
            .exported_symbols
            .difference(&new_lib.exported_symbols)
            .cloned()
            .collect();

        if !missing.is_empty() {
            return AbiCompatibility::Incompatible(AbiBreakReason::SymbolsRemoved {
                library: old_path.clone(),
                missing,
            });
        }

        if new_lib.exported_symbols.len() > old_lib.exported_symbols.len() {
            total_added_symbols += new_lib.exported_symbols.len() - old_lib.exported_symbols.len();
        }
    }

    AbiCompatibility::CompatibleSuperset {
        added_symbols: total_added_symbols,
    }
}

/// Write `.ABIINFO` TOML into the specified path.
pub fn write_abi_info(dest: &Path, abi: &PartAbi) -> Result<()> {
    let toml_str = toml::to_string_pretty(abi)
        .map_err(|e| WrightError::context("serialize .ABIINFO", e))?;
    std::fs::write(dest, toml_str)
        .map_err(|e| WrightError::context("write .ABIINFO", e))?;
    Ok(())
}

/// Read `.ABIINFO` TOML from an extracted part directory.
pub fn read_abi_info(extract_dir: &Path) -> Result<Option<PartAbi>> {
    let path = extract_dir.join(".ABIINFO");
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| WrightError::context("read .ABIINFO", e))?;
    let abi: PartAbi = toml::from_str(&content)
        .map_err(|e| WrightError::context("parse .ABIINFO", e))?;
    Ok(Some(abi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_abi_is_compatible() {
        let mut symbols = BTreeSet::new();
        symbols.insert("foo".to_string());
        symbols.insert("bar".to_string());

        let lib = ElfAbi {
            soname: Some("libtest.so.1".to_string()),
            needed: vec!["libc.so.6".to_string()],
            exported_symbols: symbols,
            abi_hash: "hash1".to_string(),
        };

        let mut old_abi = PartAbi::default();
        old_abi.libraries.insert("usr/lib/libtest.so.1".to_string(), lib.clone());
        old_abi.recompute_overall_hash();

        let new_abi = old_abi.clone();
        assert_eq!(diff_abi(&old_abi, &new_abi), AbiCompatibility::Identical);
        assert!(diff_abi(&old_abi, &new_abi).is_compatible());
    }

    #[test]
    fn superset_symbols_is_compatible() {
        let mut old_symbols = BTreeSet::new();
        old_symbols.insert("foo".to_string());

        let mut new_symbols = old_symbols.clone();
        new_symbols.insert("bar".to_string());

        let old_lib = ElfAbi {
            soname: Some("libtest.so.1".to_string()),
            needed: vec![],
            exported_symbols: old_symbols,
            abi_hash: "hash_old".to_string(),
        };

        let new_lib = ElfAbi {
            soname: Some("libtest.so.1".to_string()),
            needed: vec![],
            exported_symbols: new_symbols,
            abi_hash: "hash_new".to_string(),
        };

        let mut old_abi = PartAbi::default();
        old_abi.libraries.insert("usr/lib/libtest.so.1".to_string(), old_lib);
        old_abi.recompute_overall_hash();

        let mut new_abi = PartAbi::default();
        new_abi.libraries.insert("usr/lib/libtest.so.1".to_string(), new_lib);
        new_abi.recompute_overall_hash();

        let res = diff_abi(&old_abi, &new_abi);
        assert_eq!(res, AbiCompatibility::CompatibleSuperset { added_symbols: 1 });
        assert!(res.is_compatible());
    }

    #[test]
    fn missing_symbol_is_incompatible() {
        let mut old_symbols = BTreeSet::new();
        old_symbols.insert("foo".to_string());
        old_symbols.insert("bar".to_string());

        let mut new_symbols = BTreeSet::new();
        new_symbols.insert("foo".to_string());

        let old_lib = ElfAbi {
            soname: Some("libtest.so.1".to_string()),
            needed: vec![],
            exported_symbols: old_symbols,
            abi_hash: "hash_old".to_string(),
        };

        let new_lib = ElfAbi {
            soname: Some("libtest.so.1".to_string()),
            needed: vec![],
            exported_symbols: new_symbols,
            abi_hash: "hash_new".to_string(),
        };

        let mut old_abi = PartAbi::default();
        old_abi.libraries.insert("usr/lib/libtest.so.1".to_string(), old_lib);
        old_abi.recompute_overall_hash();

        let mut new_abi = PartAbi::default();
        new_abi.libraries.insert("usr/lib/libtest.so.1".to_string(), new_lib);
        new_abi.recompute_overall_hash();

        let res = diff_abi(&old_abi, &new_abi);
        assert!(matches!(res, AbiCompatibility::Incompatible(AbiBreakReason::SymbolsRemoved { .. })));
        assert!(!res.is_compatible());
    }

    #[test]
    fn soname_change_is_incompatible() {
        let mut symbols = BTreeSet::new();
        symbols.insert("foo".to_string());

        let old_lib = ElfAbi {
            soname: Some("libtest.so.1".to_string()),
            needed: vec![],
            exported_symbols: symbols.clone(),
            abi_hash: "hash_old".to_string(),
        };

        let new_lib = ElfAbi {
            soname: Some("libtest.so.2".to_string()),
            needed: vec![],
            exported_symbols: symbols,
            abi_hash: "hash_new".to_string(),
        };

        let mut old_abi = PartAbi::default();
        old_abi.libraries.insert("usr/lib/libtest.so.1".to_string(), old_lib);
        old_abi.recompute_overall_hash();

        let mut new_abi = PartAbi::default();
        new_abi.libraries.insert("usr/lib/libtest.so.2".to_string(), new_lib);
        new_abi.recompute_overall_hash();

        let res = diff_abi(&old_abi, &new_abi);
        assert!(matches!(res, AbiCompatibility::Incompatible(AbiBreakReason::SonameChanged { .. })));
        assert!(!res.is_compatible());
    }

    #[test]
    fn toml_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".ABIINFO");

        let mut symbols = BTreeSet::new();
        symbols.insert("func_a".to_string());
        symbols.insert("func_b".to_string());

        let lib = ElfAbi {
            soname: Some("libdemo.so.1".to_string()),
            needed: vec!["libc.so.6".to_string()],
            exported_symbols: symbols,
            abi_hash: "abc123hash".to_string(),
        };

        let mut abi = PartAbi::default();
        abi.libraries.insert("usr/lib/libdemo.so.1".to_string(), lib);
        abi.recompute_overall_hash();

        write_abi_info(&file, &abi).unwrap();
        let loaded = read_abi_info(dir.path()).unwrap().expect("loaded .ABIINFO");
        assert_eq!(abi, loaded);
    }
}

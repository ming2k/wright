//! ELF inspection helpers used by the package-time runtime-deps lint.
//!
//! Per ADR-0017 the lint never injects derived data into PARTINFO; the
//! plan source is the single source of truth. This module only exposes
//! readers — callers compare what they read against what the plan
//! declared.

use goblin::elf::Elf;
use std::path::Path;

use crate::error::{Result, WrightError};

fn is_elf_magic(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == b"\x7fELF"
}

/// Read the `DT_NEEDED` SONAMEs from an ELF file.
///
/// Returns `Ok(None)` when the file is not ELF (skip non-ELF artifacts
/// silently — staging trees contain text, scripts, configs, etc.).
/// Returns `Ok(Some(vec![]))` for ELFs with no dynamic section (fully
/// static binaries, archive members), which is meaningful: the lint
/// records that this file imposes no runtime requirements.
pub fn read_dt_needed(path: &Path) -> Result<Option<Vec<String>>> {
    let bytes = std::fs::read(path)
        .map_err(|e| WrightError::PartError(format!("read {}: {}", path.display(), e)))?;

    if !is_elf_magic(&bytes) {
        return Ok(None);
    }
    let elf = match Elf::parse(&bytes) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    Ok(Some(
        elf.libraries.iter().map(|s| (*s).to_string()).collect(),
    ))
}

/// Read the `DT_SONAME` of an ELF file.
///
/// `Ok(None)` for non-ELF, ELF without a SONAME (most executables and
/// non-versioned objects), or unreadable paths. ELF parse failures are
/// treated as "not an ELF" rather than propagated, mirroring `read_dt_needed`.
pub fn read_dt_soname(path: &Path) -> Result<Option<String>> {
    let bytes = std::fs::read(path)
        .map_err(|e| WrightError::PartError(format!("read {}: {}", path.display(), e)))?;

    if !is_elf_magic(&bytes) {
        return Ok(None);
    }
    let elf = match Elf::parse(&bytes) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    Ok(elf.soname.map(|s| s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn host_libc_candidates() -> Vec<PathBuf> {
        // Common locations where a glibc / musl share-lib lives. Tests are
        // skipped when none of these exist (e.g. on a non-ELF host).
        [
            "/lib/x86_64-linux-gnu/libc.so.6",
            "/lib64/libc.so.6",
            "/lib/libc.so.6",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }

    fn first_existing(paths: &[PathBuf]) -> Option<&Path> {
        paths.iter().find(|p| p.exists()).map(|p| p.as_path())
    }

    #[test]
    fn non_elf_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notelf.txt");
        std::fs::write(&p, b"hello world\n").unwrap();
        assert!(read_dt_needed(&p).unwrap().is_none());
        assert!(read_dt_soname(&p).unwrap().is_none());
    }

    #[test]
    fn libc_has_soname_and_no_unexpected_panic() {
        let candidates = host_libc_candidates();
        let Some(libc) = first_existing(&candidates) else {
            eprintln!("skipping: no host libc found at known paths");
            return;
        };
        let soname = read_dt_soname(libc).unwrap();
        assert!(
            soname.is_some(),
            "libc.so.6 should carry a DT_SONAME tag (got None)"
        );
        // libc.so.6's own DT_NEEDED is small but non-panicking is the point.
        let needed = read_dt_needed(libc).unwrap();
        assert!(needed.is_some(), "libc.so.6 parses as ELF");
    }

    #[test]
    fn missing_path_is_io_error() {
        let p = PathBuf::from("/definitely/does/not/exist/elf");
        assert!(read_dt_needed(&p).is_err());
        assert!(read_dt_soname(&p).is_err());
    }
}

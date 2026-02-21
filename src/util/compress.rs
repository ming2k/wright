use std::io::Read;
use std::path::{Component, Path};

use crate::error::{WrightError, Result};
use tracing::warn;

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};

pub fn compress_zstd(input: &Path, output: &Path) -> Result<()> {
    let input_data = std::fs::read(input).map_err(|e| {
        WrightError::ArchiveError(format!("failed to read {}: {}", input.display(), e))
    })?;

    let compressed = zstd::encode_all(input_data.as_slice(), 19).map_err(|e| {
        WrightError::ArchiveError(format!("zstd compression failed: {}", e))
    })?;

    std::fs::write(output, compressed).map_err(|e| {
        WrightError::ArchiveError(format!("failed to write {}: {}", output.display(), e))
    })?;

    Ok(())
}

pub fn decompress_zstd(input: &Path, output: &Path) -> Result<()> {
    let input_data = std::fs::read(input).map_err(|e| {
        WrightError::ArchiveError(format!("failed to read {}: {}", input.display(), e))
    })?;

    let mut decoder = zstd::Decoder::new(input_data.as_slice()).map_err(|e| {
        WrightError::ArchiveError(format!("zstd decompression init failed: {}", e))
    })?;

    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).map_err(|e| {
        WrightError::ArchiveError(format!("zstd decompression failed: {}", e))
    })?;

    std::fs::write(output, decompressed).map_err(|e| {
        WrightError::ArchiveError(format!("failed to write {}: {}", output.display(), e))
    })?;

    Ok(())
}

/// Create a tar.zst archive from a directory.
/// Handles symlinks by archiving them as symlinks (not following them).
pub fn create_tar_zst(source_dir: &Path, output_path: &Path) -> Result<()> {
    let file = std::fs::File::create(output_path).map_err(|e| {
        WrightError::ArchiveError(format!("failed to create {}: {}", output_path.display(), e))
    })?;

    let encoder = zstd::Encoder::new(file, 3).map_err(|e| {
        WrightError::ArchiveError(format!("zstd encoder init failed: {}", e))
    })?;

    let mut tar_builder = tar::Builder::new(encoder);
    tar_builder.follow_symlinks(false);

    for entry in walkdir::WalkDir::new(source_dir).sort_by_file_name() {
        let entry = entry.map_err(|e| {
            WrightError::ArchiveError(format!("failed to walk directory: {}", e))
        })?;
        let full_path = entry.path();
        let raw_rel_path = full_path.strip_prefix(source_dir).unwrap_or(full_path);
        // The root entry itself produces an empty relative path — skip silently.
        if raw_rel_path == std::path::Path::new("") {
            continue;
        }
        let Some(rel_path) = normalize_archive_path(raw_rel_path) else {
            warn!(
                "Skipping unsafe archive path: {} (source: {})",
                raw_rel_path.display(),
                full_path.display()
            );
            continue;
        };

        let metadata = entry.path().symlink_metadata().map_err(|e| {
            WrightError::ArchiveError(format!("failed to read metadata for {}: {}", full_path.display(), e))
        })?;

        if metadata.is_symlink() {
            let target = std::fs::read_link(full_path).map_err(|e| {
                WrightError::ArchiveError(format!("failed to read symlink {}: {}", full_path.display(), e))
            })?;
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_mtime(metadata.modified()
                .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs())
                .unwrap_or(0));
            tar_builder.append_link(&mut header, &rel_path, &target).map_err(|e| {
                WrightError::ArchiveError(format!("tar append symlink failed: {}", e))
            })?;
        } else if metadata.is_dir() {
            tar_builder.append_dir(&rel_path, full_path).map_err(|e| {
                WrightError::ArchiveError(format!("tar append dir failed: {}", e))
            })?;
        } else {
            // The tar crate's append_path_with_name passes the absolute source path
            // to append_special for device/FIFO entries, ignoring the archive name.
            // Handle special files explicitly to avoid that bug.
            #[cfg(unix)]
            {
                let file_type = metadata.file_type();
                if file_type.is_socket() {
                    // Sockets cannot be archived.
                    warn!("Skipping socket in archive: {}", rel_path.display());
                } else if file_type.is_fifo()
                    || file_type.is_char_device()
                    || file_type.is_block_device()
                {
                    let mut header = tar::Header::new_gnu();
                    header.set_path(&rel_path).map_err(|e| {
                        WrightError::ArchiveError(format!(
                            "tar set path failed for {}: {}",
                            rel_path.display(), e
                        ))
                    })?;
                    header.set_mode(metadata.mode());
                    header.set_uid(metadata.uid() as u64);
                    header.set_gid(metadata.gid() as u64);
                    header.set_size(0);
                    header.set_mtime(metadata.modified()
                        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs())
                        .unwrap_or(0));
                    if file_type.is_fifo() {
                        header.set_entry_type(tar::EntryType::Fifo);
                    } else {
                        let dev_id = metadata.rdev();
                        let dev_major = ((dev_id >> 32) & 0xffff_f000) | ((dev_id >> 8) & 0x0000_0fff);
                        let dev_minor = ((dev_id >> 12) & 0xffff_ff00) | (dev_id & 0x0000_00ff);
                        if file_type.is_char_device() {
                            header.set_entry_type(tar::EntryType::Char);
                        } else {
                            header.set_entry_type(tar::EntryType::Block);
                        }
                        header.set_device_major(dev_major as u32).map_err(|e| {
                            WrightError::ArchiveError(format!("tar set device major failed: {}", e))
                        })?;
                        header.set_device_minor(dev_minor as u32).map_err(|e| {
                            WrightError::ArchiveError(format!("tar set device minor failed: {}", e))
                        })?;
                    }
                    header.set_cksum();
                    tar_builder.append(&header, std::io::empty()).map_err(|e| {
                        WrightError::ArchiveError(format!(
                            "tar append special failed for {}: {}",
                            rel_path.display(), e
                        ))
                    })?;
                } else {
                    tar_builder.append_path_with_name(full_path, &rel_path).map_err(|e| {
                        WrightError::ArchiveError(format!("tar append file failed: {}", e))
                    })?;
                }
            }
            #[cfg(not(unix))]
            {
                tar_builder.append_path_with_name(full_path, &rel_path).map_err(|e| {
                    WrightError::ArchiveError(format!("tar append file failed: {}", e))
                })?;
            }
        }
    }

    let encoder = tar_builder
        .into_inner()
        .map_err(|e| WrightError::ArchiveError(format!("tar finalize failed: {}", e)))?;

    encoder
        .finish()
        .map_err(|e| WrightError::ArchiveError(format!("zstd finish failed: {}", e)))?;

    Ok(())
}

/// Normalize a filesystem path into a safe, relative path for archive entry names.
fn normalize_archive_path(path: &Path) -> Option<std::path::PathBuf> {
    let mut normalized = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(seg) => normalized.push(seg),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => return None,
        }
    }
    if normalized.as_os_str().is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Extract a tar.zst archive to a directory.
pub fn extract_tar_zst(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path).map_err(|e| {
        WrightError::ArchiveError(format!("failed to open {}: {}", archive_path.display(), e))
    })?;

    let decoder = zstd::Decoder::new(file).map_err(|e| {
        WrightError::ArchiveError(format!("zstd decoder init failed: {}", e))
    })?;

    let archive = tar::Archive::new(decoder);
    unpack_tar_safely(archive, dest_dir)?;

    Ok(())
}

/// Generic extraction function that supports .tar.gz, .tar.xz, and .tar.zst
pub fn extract_archive(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let filename = archive_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    
    if filename.ends_with(".tar.zst") {
        extract_tar_zst(archive_path, dest_dir)
    } else if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        extract_tar_gz(archive_path, dest_dir)
    } else if filename.ends_with(".tar.xz") {
        extract_tar_xz(archive_path, dest_dir)
    } else if filename.ends_with(".tar.bz2") {
        extract_tar_bz2(archive_path, dest_dir)
    } else {
        Err(WrightError::ArchiveError(format!("unsupported archive format: {}", filename)))
    }
}

pub fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    let file = std::fs::File::open(archive_path).map_err(WrightError::IoError)?;
    let decoder = GzDecoder::new(file);
    let archive = tar::Archive::new(decoder);
    unpack_tar_safely(archive, dest_dir)?;
    Ok(())
}

pub fn extract_tar_bz2(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    use bzip2::read::BzDecoder;
    let file = std::fs::File::open(archive_path).map_err(WrightError::IoError)?;
    let decoder = BzDecoder::new(file);
    let archive = tar::Archive::new(decoder);
    unpack_tar_safely(archive, dest_dir)?;
    Ok(())
}

pub fn extract_tar_xz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    use xz2::read::XzDecoder;
    let file = std::fs::File::open(archive_path).map_err(WrightError::IoError)?;
    let decoder = XzDecoder::new(file);
    let archive = tar::Archive::new(decoder);
    unpack_tar_safely(archive, dest_dir)?;
    Ok(())
}

fn is_path_safe(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return false;
        }
    }
    true
}

fn unpack_tar_safely<R: Read>(mut archive: tar::Archive<R>, dest_dir: &Path) -> Result<()> {
    for entry in archive.entries().map_err(|e| {
        WrightError::ArchiveError(format!("failed to read archive entries: {}", e))
    })? {
        let mut entry = entry.map_err(|e| {
            WrightError::ArchiveError(format!("failed to read tar entry: {}", e))
        })?;

        let path = entry.path().map_err(|e| {
            WrightError::ArchiveError(format!("failed to read entry path: {}", e))
        })?;

        if !is_path_safe(&path) {
            return Err(WrightError::ArchiveError(format!(
                "unsafe path in archive: {}",
                path.to_string_lossy()
            )));
        }

        // The tar crate's unpack_in strips setuid/setgid/sticky bits (a security
        // measure). Capture the full mode from the header beforehand so we can
        // re-apply it afterwards, preserving bits like the setuid on unix_chkpwd.
        #[cfg(unix)]
        let restore = {
            let mode = entry.header().mode().ok();
            let is_file = matches!(
                entry.header().entry_type(),
                tar::EntryType::Regular | tar::EntryType::GNUSparse
            );
            let dest = dest_dir.join(&*path);
            (mode, is_file, dest)
        };

        entry.unpack_in(dest_dir).map_err(|e| {
            WrightError::ArchiveError(format!("tar extract failed: {}", e))
        })?;

        #[cfg(unix)]
        {
            let (mode, is_file, dest) = restore;
            if is_file {
                if let Some(m) = mode {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &dest,
                        std::fs::Permissions::from_mode(m),
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn create_tar_zst_with_path(path: &str) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = std::fs::File::create(tmp.path()).unwrap();
        let mut encoder = zstd::Encoder::new(file, 3).unwrap();
        let tar = build_raw_tar(path, b"evil");
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap();
        tmp
    }

    fn create_tar_gz_with_path(path: &str) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = std::fs::File::create(tmp.path()).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let tar = build_raw_tar(path, b"evil");
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap();
        tmp
    }

    fn create_tar_xz_with_path(path: &str) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = std::fs::File::create(tmp.path()).unwrap();
        let mut encoder = xz2::write::XzEncoder::new(file, 6);
        let tar = build_raw_tar(path, b"evil");
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap();
        tmp
    }

    fn create_tar_bz2_with_path(path: &str) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let file = std::fs::File::create(tmp.path()).unwrap();
        let mut encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
        let tar = build_raw_tar(path, b"evil");
        encoder.write_all(&tar).unwrap();
        encoder.finish().unwrap();
        tmp
    }

    fn build_raw_tar(path: &str, data: &[u8]) -> Vec<u8> {
        fn write_octal(dst: &mut [u8], value: u64) {
            let s = format!("{:o}", value);
            let start = dst.len().saturating_sub(s.len() + 1);
            for b in dst.iter_mut() {
                *b = b'0';
            }
            dst[dst.len() - 1] = 0;
            dst[start..start + s.len()].copy_from_slice(s.as_bytes());
        }

        let mut header = [0u8; 512];
        let name_bytes = path.as_bytes();
        let name_len = name_bytes.len().min(100);
        header[0..name_len].copy_from_slice(&name_bytes[..name_len]);
        write_octal(&mut header[100..108], 0o644);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], data.len() as u64);
        write_octal(&mut header[136..148], 0);
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");

        for b in header[148..156].iter_mut() {
            *b = b' ';
        }
        let checksum: u32 = header.iter().map(|b| *b as u32).sum();
        let checksum_str = format!("{:06o}\0 ", checksum);
        header[148..156].copy_from_slice(checksum_str.as_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        let pad = (512 - (data.len() % 512)) % 512;
        out.extend(std::iter::repeat(0u8).take(pad));
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    #[test]
    fn test_extract_rejects_parent_dir_paths() {
        let archive = create_tar_zst_with_path("../evil.txt");
        let dest = tempfile::tempdir().unwrap();

        let result = extract_tar_zst(archive.path(), dest.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_rejects_absolute_paths() {
        let archive = create_tar_zst_with_path("/evil.txt");
        let dest = tempfile::tempdir().unwrap();

        let result = extract_tar_zst(archive.path(), dest.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_gz_rejects_parent_dir_paths() {
        let archive = create_tar_gz_with_path("../evil.txt");
        let dest = tempfile::tempdir().unwrap();

        let result = extract_tar_gz(archive.path(), dest.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_xz_rejects_parent_dir_paths() {
        let archive = create_tar_xz_with_path("../evil.txt");
        let dest = tempfile::tempdir().unwrap();

        let result = extract_tar_xz(archive.path(), dest.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_bz2_rejects_parent_dir_paths() {
        let archive = create_tar_bz2_with_path("../evil.txt");
        let dest = tempfile::tempdir().unwrap();

        let result = extract_tar_bz2(archive.path(), dest.path());
        assert!(result.is_err());
    }
}

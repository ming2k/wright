use std::io::Read;
use std::path::Path;

use crate::error::{WrightError, Result};

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

    let encoder = zstd::Encoder::new(file, 19).map_err(|e| {
        WrightError::ArchiveError(format!("zstd encoder init failed: {}", e))
    })?;

    let mut tar_builder = tar::Builder::new(encoder);
    tar_builder.follow_symlinks(false);

    for entry in walkdir::WalkDir::new(source_dir).sort_by_file_name() {
        let entry = entry.map_err(|e| {
            WrightError::ArchiveError(format!("failed to walk directory: {}", e))
        })?;
        let full_path = entry.path();
        let rel_path = full_path.strip_prefix(source_dir).unwrap_or(full_path);
        if rel_path == Path::new("") {
            continue;
        }

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
            tar_builder.append_link(&mut header, rel_path, &target).map_err(|e| {
                WrightError::ArchiveError(format!("tar append symlink failed: {}", e))
            })?;
        } else if metadata.is_dir() {
            tar_builder.append_dir(rel_path, full_path).map_err(|e| {
                WrightError::ArchiveError(format!("tar append dir failed: {}", e))
            })?;
        } else {
            tar_builder.append_path_with_name(full_path, rel_path).map_err(|e| {
                WrightError::ArchiveError(format!("tar append file failed: {}", e))
            })?;
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

/// Extract a tar.zst archive to a directory.
pub fn extract_tar_zst(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path).map_err(|e| {
        WrightError::ArchiveError(format!("failed to open {}: {}", archive_path.display(), e))
    })?;

    let decoder = zstd::Decoder::new(file).map_err(|e| {
        WrightError::ArchiveError(format!("zstd decoder init failed: {}", e))
    })?;

    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest_dir).map_err(|e| {
        WrightError::ArchiveError(format!("tar extract failed: {}", e))
    })?;

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
    let file = std::fs::File::open(archive_path).map_err(|e| WrightError::IoError(e))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest_dir).map_err(|e| {
        WrightError::ArchiveError(format!("tar.gz extract failed: {}", e))
    })?;
    Ok(())
}

pub fn extract_tar_bz2(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    use bzip2::read::BzDecoder;
    let file = std::fs::File::open(archive_path).map_err(|e| WrightError::IoError(e))?;
    let decoder = BzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest_dir).map_err(|e| {
        WrightError::ArchiveError(format!("tar.bz2 extract failed: {}", e))
    })?;
    Ok(())
}

pub fn extract_tar_xz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    use xz2::read::XzDecoder;
    let file = std::fs::File::open(archive_path).map_err(|e| WrightError::IoError(e))?;
    let decoder = XzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dest_dir).map_err(|e| {
        WrightError::ArchiveError(format!("tar.xz extract failed: {}", e))
    })?;
    Ok(())
}

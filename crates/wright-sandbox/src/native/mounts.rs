use std::path::{Path, PathBuf};

pub(super) fn prepare_mount_destination(
    newroot: &Path,
    source: &Path,
    target: &Path,
) -> std::result::Result<PathBuf, String> {
    let relative = target
        .strip_prefix("/")
        .map_err(|_| format!("mount target must be absolute: {}", target.display()))?;
    let parent = relative
        .parent()
        .ok_or_else(|| format!("mount target has no parent: {}", target.display()))?;

    let mut current = newroot.to_path_buf();
    for component in parent.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(format!(
                "mount target contains an unsafe component: {}",
                target.display()
            ));
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "mount target parent is a symlink: {}",
                    current.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "mount target parent is not a directory: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)
                    .map_err(|error| format!("mkdir {}: {error}", current.display()))?;
            }
            Err(error) => {
                return Err(format!("inspect {}: {error}", current.display()));
            }
        }
    }

    let destination = newroot.join(relative);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() {
            std::fs::remove_file(&destination)
                .map_err(|error| format!("unlink {}: {error}", destination.display()))?;
        } else if source.is_dir() != metadata.is_dir() {
            return Err(format!(
                "mount source and target types differ: {} -> {}",
                source.display(),
                destination.display()
            ));
        } else {
            return Ok(destination);
        }
    }

    if source.is_dir() {
        std::fs::create_dir(&destination)
            .map_err(|error| format!("mkdir {}: {error}", destination.display()))?;
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|error| format!("touch {}: {error}", destination.display()))?;
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn mount_destination_rejects_symlinked_parent() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();

        let error =
            prepare_mount_destination(root.path(), source.path(), Path::new("/escape/target"))
                .unwrap_err();
        assert!(error.contains("parent is a symlink"));
        assert!(!outside.path().join("target").exists());
    }
}

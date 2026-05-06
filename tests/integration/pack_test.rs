use std::path::PathBuf;

use wright::part::pack::{self, PackManifest, PackMeta, PackOrigin, PackPart, PACK_MANIFEST_NAME};

/// Build a minimal "part" archive on disk. We don't need a real Wright build to
/// exercise pack integrity — a tar.zst with a valid `.PARTINFO` is enough.
fn write_dummy_part_archive(out_dir: &std::path::Path, name: &str) -> PathBuf {
    let part_dir = tempfile::tempdir().unwrap();
    let bin = part_dir.path().join("usr/bin").join(name);
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, format!("#!/bin/sh\necho {}\n", name)).unwrap();
    std::fs::write(
        part_dir.path().join(".PARTINFO"),
        format!(
            r#"[part]
name = "{name}"
build_date = "2026-01-01T00:00:00Z"

[plan]
name = "{name}"
version = "1.0.0"
release = 1
description = "dummy"
arch = "x86_64"
license = "MIT"
"#,
            name = name
        ),
    )
    .unwrap();

    let archive_path = out_dir.join(format!("{}-1.0.0-1-x86_64.wright.tar.zst", name));
    wright::util::compress::create_tar_zst(part_dir.path(), &archive_path).unwrap();
    archive_path
}

#[test]
fn pack_round_trip_preserves_manifest_and_part_hashes() {
    let work = tempfile::tempdir().unwrap();
    let parts_dir = work.path().join("parts");
    std::fs::create_dir_all(&parts_dir).unwrap();

    let foo_archive = write_dummy_part_archive(&parts_dir, "foo");
    let bar_archive = write_dummy_part_archive(&parts_dir, "bar");

    let manifest = PackManifest {
        pack: PackMeta {
            name: "round-trip".into(),
            version: "1".into(),
            description: "test".into(),
            arch: "x86_64".into(),
        },
        parts: vec![
            PackPart {
                file: format!(
                    "parts/{}",
                    foo_archive.file_name().unwrap().to_string_lossy()
                ),
                origin: PackOrigin::Manual,
                sha256: None,
            },
            PackPart {
                file: format!(
                    "parts/{}",
                    bar_archive.file_name().unwrap().to_string_lossy()
                ),
                origin: PackOrigin::Dependency,
                sha256: None,
            },
        ],
        assumes: Vec::new(),
        config: None,
        overlay_sha256: None,
    };
    std::fs::write(
        work.path().join(PACK_MANIFEST_NAME),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let pack_path = work.path().join("round-trip.wright.pack.tar");
    pack::create_pack(work.path(), &pack_path).unwrap();

    let read_back = pack::read_manifest(&pack_path).unwrap();
    assert_eq!(read_back.pack.name, "round-trip");
    assert_eq!(read_back.parts.len(), 2);
    for part in &read_back.parts {
        assert!(part.sha256.is_some(), "manifest must record sha256");
    }
    assert_eq!(read_back.parts[1].origin, PackOrigin::Dependency);

    // Extract and verify integrity end-to-end.
    let extract = tempfile::tempdir().unwrap();
    pack::extract_pack(&pack_path, extract.path()).unwrap();
    let mismatches = pack::verify_extracted_pack(&read_back, extract.path()).unwrap();
    assert!(
        mismatches.is_empty(),
        "expected clean pack, got mismatches: {:?}",
        mismatches
    );
}

#[test]
fn pack_overlay_round_trip_records_hash_and_applies_to_root() {
    use std::collections::HashSet;

    let work = tempfile::tempdir().unwrap();
    let parts_dir = work.path().join("parts");
    std::fs::create_dir_all(&parts_dir).unwrap();
    let _foo = write_dummy_part_archive(&parts_dir, "foo");

    // Overlay carries a base config file.
    let overlay_dir = work.path().join("overlay");
    std::fs::create_dir_all(overlay_dir.join("etc")).unwrap();
    std::fs::write(overlay_dir.join("etc/motd"), "welcome aboard\n").unwrap();

    let manifest = PackManifest {
        pack: PackMeta {
            name: "overlay-test".into(),
            version: "1".into(),
            description: "test".into(),
            arch: "x86_64".into(),
        },
        parts: vec![PackPart {
            file: "parts/foo-1.0.0-1-x86_64.wright.tar.zst".into(),
            origin: PackOrigin::Manual,
            sha256: None,
        }],
        assumes: Vec::new(),
        config: None,
        overlay_sha256: None,
    };
    std::fs::write(
        work.path().join(PACK_MANIFEST_NAME),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let pack_path = work.path().join("overlay-test.wright.pack.tar");
    pack::create_pack(work.path(), &pack_path).unwrap();

    let read_back = pack::read_manifest(&pack_path).unwrap();
    assert!(
        read_back.overlay_sha256.is_some(),
        "manifest must record overlay sha256"
    );

    // Apply overlay into a fresh root, simulating the launch path.
    let extract = tempfile::tempdir().unwrap();
    pack::extract_pack(&pack_path, extract.path()).unwrap();
    let target = tempfile::tempdir().unwrap();
    let written = pack::apply_overlay_tar(
        &extract.path().join("overlay.tar"),
        target.path(),
        &HashSet::new(),
    )
    .unwrap();
    assert!(
        written.iter().any(|p| p == "/etc/motd"),
        "overlay should report /etc/motd: {:?}",
        written
    );
    let motd = std::fs::read_to_string(target.path().join("etc/motd")).unwrap();
    assert_eq!(motd, "welcome aboard\n");
}

#[test]
fn pack_overlay_skips_paths_owned_by_installed_parts() {
    use std::collections::HashSet;

    let work = tempfile::tempdir().unwrap();
    let parts_dir = work.path().join("parts");
    std::fs::create_dir_all(&parts_dir).unwrap();
    let _foo = write_dummy_part_archive(&parts_dir, "foo");

    let overlay_dir = work.path().join("overlay");
    std::fs::create_dir_all(overlay_dir.join("etc")).unwrap();
    std::fs::write(overlay_dir.join("etc/skip.conf"), "from overlay\n").unwrap();

    let manifest = PackManifest {
        pack: PackMeta {
            name: "skip-test".into(),
            version: "1".into(),
            description: "test".into(),
            arch: "x86_64".into(),
        },
        parts: vec![PackPart {
            file: "parts/foo-1.0.0-1-x86_64.wright.tar.zst".into(),
            origin: PackOrigin::Manual,
            sha256: None,
        }],
        assumes: Vec::new(),
        config: None,
        overlay_sha256: None,
    };
    std::fs::write(
        work.path().join(PACK_MANIFEST_NAME),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let pack_path = work.path().join("skip-test.wright.pack.tar");
    pack::create_pack(work.path(), &pack_path).unwrap();

    let extract = tempfile::tempdir().unwrap();
    pack::extract_pack(&pack_path, extract.path()).unwrap();

    let target = tempfile::tempdir().unwrap();
    let mut owned = HashSet::new();
    owned.insert("/etc/skip.conf".to_string());
    let written =
        pack::apply_overlay_tar(&extract.path().join("overlay.tar"), target.path(), &owned)
            .unwrap();
    assert!(
        !written.iter().any(|p| p == "/etc/skip.conf"),
        "overlay must not clobber a path owned by an installed part: {:?}",
        written
    );
    assert!(!target.path().join("etc/skip.conf").exists());
}

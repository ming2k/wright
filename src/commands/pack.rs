use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::cli::launch::PackArgs;
use crate::part::pack;

pub async fn execute_pack(args: PackArgs) -> Result<()> {
    if args.inspect {
        return inspect(&args.path);
    }
    build(args).await
}

async fn build(args: PackArgs) -> Result<()> {
    let source_dir = args.path;
    let manifest_path = source_dir.join(pack::PACK_MANIFEST_NAME);
    let raw = std::fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "failed to read {} from {}",
            pack::PACK_MANIFEST_NAME,
            source_dir.display()
        )
    })?;
    let manifest = pack::parse_manifest(&raw).context("invalid pack.toml")?;

    let output = args.output.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(pack::default_output_filename(&manifest))
    });

    let written = pack::create_pack(&source_dir, &output)
        .with_context(|| format!("failed to create pack {}", output.display()))?;

    println!(
        "wrote {} ({} parts{})",
        written.display(),
        manifest.parts.len(),
        if source_dir.join(pack::PACK_OVERLAY_DIR).is_dir() {
            ", overlay included"
        } else {
            ""
        }
    );
    Ok(())
}

fn inspect(pack_path: &std::path::Path) -> Result<()> {
    let manifest = pack::read_manifest(pack_path)
        .with_context(|| format!("failed to read pack {}", pack_path.display()))?;

    println!("Pack        : {}", manifest.pack.name);
    println!("Version     : {}", manifest.pack.version);
    if !manifest.pack.description.is_empty() {
        println!("Description : {}", manifest.pack.description);
    }
    if !manifest.pack.arch.is_empty() {
        println!("Arch        : {}", manifest.pack.arch);
    }
    println!("Parts       : {}", manifest.parts.len());
    for (bucket, count) in pack::summarize_by_dir(&manifest) {
        println!("  {}: {}", bucket, count);
    }
    if !manifest.assumes.is_empty() {
        println!("Assumes     : {}", manifest.assumes.len());
        for a in &manifest.assumes {
            println!("  {} {}", a.name, a.version);
        }
    }
    if let Some(ref cfg) = manifest.config {
        println!("Config      :");
        if let Some(ref h) = cfg.hostname {
            println!("  hostname = {}", h);
        }
        if let Some(ref t) = cfg.timezone {
            println!("  timezone = {}", t);
        }
        if let Some(ref l) = cfg.locale {
            println!("  locale   = {}", l);
        }
        if !cfg.services.is_empty() {
            println!("  services = {}", cfg.services.join(", "));
        }
    }
    if let Some(ref h) = manifest.overlay_sha256 {
        println!("Overlay     : sha256 {}", h);
    } else {
        println!("Overlay     : none");
    }
    Ok(())
}

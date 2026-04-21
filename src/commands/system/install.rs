use super::apply::{collect_install_args, resolve_install_paths};
use crate::archive::resolver::LocalResolver;
use crate::database::InstalledDb;
use crate::transaction;
use anyhow::Result;
use std::path::Path;

pub async fn execute_install(
    db: &InstalledDb,
    parts: Vec<String>,
    force: bool,
    nodeps: bool,
    root_dir: &Path,
    resolver: &LocalResolver,
) -> Result<()> {
    use std::io::IsTerminal;
    let parts = collect_install_args(parts)?;
    if parts.is_empty() {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("no part paths received from stdin; did the build succeed?");
        }
        anyhow::bail!("no parts specified (pass part names/paths as arguments or via stdin)");
    }
    let part_paths = resolve_install_paths(resolver, &parts).await?;

    match transaction::install_parts(db, &part_paths, root_dir, resolver, force, nodeps).await {
        Ok(()) => println!("installation completed successfully"),
        Err(e) => {
            eprintln!("error: {:#}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

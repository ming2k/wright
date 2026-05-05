use super::apply::{collect_install_args, resolve_install_paths};
use crate::database::InstalledDb;
use crate::part::store::LocalPartStore;
use crate::transaction;
use anyhow::Result;
use std::path::Path;

pub async fn execute_install(
    db: &InstalledDb,
    parts: Vec<String>,
    force: bool,
    nodeps: bool,
    root_dir: &Path,
    part_store: &LocalPartStore,
) -> Result<()> {
    use std::io::IsTerminal;
    let parts = collect_install_args(parts)?;
    if parts.is_empty() {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("no part paths received from stdin; did the build succeed?");
        }
        anyhow::bail!("no parts specified (pass part names/paths as arguments or via stdin)");
    }
    let part_paths = resolve_install_paths(part_store, &parts).await?;

    match transaction::install_parts(db, &part_paths, root_dir, part_store, force, nodeps).await {
        Ok(()) => println!("installation completed successfully"),
        Err(e) => {
            eprintln!("error: {:#}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

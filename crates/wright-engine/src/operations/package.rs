use crate::config::GlobalConfig;
use crate::error::Result;

pub async fn execute_package(
    plans: &[String],
    print_parts: bool,
    force: bool,
    config: &GlobalConfig,
) -> Result<()> {
    let index = super::targets::plan_index(config)?;

    for target in plans {
        let manifest = super::targets::load_manifest(target, &index)?;

        crate::cli_action!("Sealing", "{}", manifest.metadata.name);
        crate::seal::package_manifest(&manifest, config, print_parts, force).await?;
    }

    Ok(())
}

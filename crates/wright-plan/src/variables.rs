//! Expansion of metadata variables embedded in plan values.

use crate::manifest::PlanManifest;

pub fn expand_metadata(input: &str, manifest: &PlanManifest) -> String {
    let values = [
        ("NAME", manifest.metadata.name.as_str()),
        (
            "VERSION",
            manifest.metadata.version.as_deref().unwrap_or_default(),
        ),
        ("ARCH", manifest.metadata.arch.as_str()),
    ];

    let mut expanded = input.to_string();
    for (name, value) in values {
        expanded = expanded.replace(&format!("${{{name}}}"), value);
    }
    expanded.replace("${RELEASE}", &manifest.metadata.release.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_plan_metadata() {
        let manifest = PlanManifest::parse(
            r#"
name = "hello"
version = "1.2.3"
release = 4
description = "test"
license = "MIT"
arch = "x86_64"
"#,
        )
        .unwrap();

        assert_eq!(
            expand_metadata("${NAME}-${VERSION}-${RELEASE}-${ARCH}", &manifest),
            "hello-1.2.3-4-x86_64"
        );
    }
}

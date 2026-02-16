use std::collections::HashMap;

/// Substitute `${VAR_NAME}` patterns in a script with values from the vars map.
pub fn substitute(script: &str, vars: &HashMap<String, String>) -> String {
    let mut result = script.to_string();
    for (key, value) in vars {
        let pattern = format!("${{{}}}", key);
        result = result.replace(&pattern, value);
    }
    result
}

/// Build the standard variable map for a build context.
pub fn standard_variables(
    pkg_name: &str,
    pkg_version: &str,
    pkg_release: u32,
    pkg_arch: &str,
    src_dir: &str,
    pkg_dir: &str,
    files_dir: &str,
    nproc: u32,
    cflags: &str,
    cxxflags: &str,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    vars.insert("PKG_NAME".to_string(), pkg_name.to_string());
    vars.insert("PKG_VERSION".to_string(), pkg_version.to_string());
    vars.insert("PKG_RELEASE".to_string(), pkg_release.to_string());
    vars.insert("PKG_ARCH".to_string(), pkg_arch.to_string());
    vars.insert("SRC_DIR".to_string(), src_dir.to_string());
    vars.insert("PKG_DIR".to_string(), pkg_dir.to_string());
    vars.insert("FILES_DIR".to_string(), files_dir.to_string());
    vars.insert("NPROC".to_string(), nproc.to_string());
    vars.insert("CFLAGS".to_string(), cflags.to_string());
    vars.insert("CXXFLAGS".to_string(), cxxflags.to_string());
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_basic() {
        let mut vars = HashMap::new();
        vars.insert("PKG_NAME".to_string(), "hello".to_string());
        vars.insert("PKG_VERSION".to_string(), "1.0.0".to_string());

        let script = "echo ${PKG_NAME}-${PKG_VERSION}";
        assert_eq!(substitute(script, &vars), "echo hello-1.0.0");
    }

    #[test]
    fn test_substitute_no_match() {
        let vars = HashMap::new();
        let script = "echo ${UNKNOWN_VAR}";
        assert_eq!(substitute(script, &vars), "echo ${UNKNOWN_VAR}");
    }

    #[test]
    fn test_substitute_multiple_occurrences() {
        let mut vars = HashMap::new();
        vars.insert("PKG_DIR".to_string(), "/tmp/pkg".to_string());

        let script = "install -d ${PKG_DIR}/usr/bin\ninstall -m755 foo ${PKG_DIR}/usr/bin/foo";
        let result = substitute(script, &vars);
        assert_eq!(
            result,
            "install -d /tmp/pkg/usr/bin\ninstall -m755 foo /tmp/pkg/usr/bin/foo"
        );
    }

    #[test]
    fn test_standard_variables() {
        let vars = standard_variables(
            "hello", "1.0.0", 1, "x86_64", "/tmp/src", "/tmp/pkg", "/tmp/patches", 4,
            "-O2", "-O2",
        );
        assert_eq!(vars["PKG_NAME"], "hello");
        assert_eq!(vars["PKG_VERSION"], "1.0.0");
        assert_eq!(vars["PKG_RELEASE"], "1");
        assert_eq!(vars["NPROC"], "4");
        assert!(!vars.contains_key("version"));
    }
}

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

/// Insert standard metadata variables.
pub fn insert_metadata_variables(
    vars: &mut HashMap<String, String>,
    name: &str,
    version: &str,
    release: u32,
    arch: &str,
) {
    vars.insert("NAME".to_string(), name.to_string());
    vars.insert("VERSION".to_string(), version.to_string());
    vars.insert("RELEASE".to_string(), release.to_string());
    vars.insert("ARCH".to_string(), arch.to_string());
}

/// Context for building standard variables.
pub struct VariableContext<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: u32,
    pub arch: &'a str,
    pub workdir: &'a str,
    pub part_dir: &'a str,
    pub main_part_name: &'a str,
    pub main_part_dir: &'a str,
}

/// Build the standard variable map for a build context.
pub fn standard_variables(ctx: VariableContext) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    insert_metadata_variables(&mut vars, ctx.name, ctx.version, ctx.release, ctx.arch);
    vars.insert("WORKDIR".to_string(), ctx.workdir.to_string());
    vars.insert("STAGING_DIR".to_string(), ctx.part_dir.to_string());
    vars.insert("MAIN_PART_NAME".to_string(), ctx.main_part_name.to_string());
    vars.insert("MAIN_STAGING_DIR".to_string(), ctx.main_part_dir.to_string());
    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_basic() {
        let mut vars = HashMap::new();
        insert_metadata_variables(&mut vars, "hello", "1.0.0", 1, "x86_64");

        let script = "echo ${NAME}-${VERSION}";
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
        vars.insert("STAGING_DIR".to_string(), "/tmp/staging".to_string());

        let script = "install -d ${STAGING_DIR}/usr/bin\ninstall -m755 foo ${STAGING_DIR}/usr/bin/foo";
        let result = substitute(script, &vars);
        assert_eq!(
            result,
            "install -d /tmp/staging/usr/bin\ninstall -m755 foo /tmp/staging/usr/bin/foo"
        );
    }

    #[test]
    fn test_standard_variables() {
        let ctx = VariableContext {
            name: "hello",
            version: "1.0.0",
            release: 1,
            arch: "x86_64",
            workdir: "/tmp/src",
            part_dir: "/tmp/staging",
            main_part_name: "hello",
            main_part_dir: "/tmp/staging",
        };
        let vars = standard_variables(ctx);
        assert_eq!(vars["NAME"], "hello");
        assert_eq!(vars["VERSION"], "1.0.0");
        assert_eq!(vars["RELEASE"], "1");
        assert_eq!(vars["ARCH"], "x86_64");
        assert_eq!(vars["WORKDIR"], "/tmp/src");
        assert_eq!(vars["MAIN_PART_NAME"], "hello");
        assert_eq!(vars["MAIN_STAGING_DIR"], "/tmp/staging");
        assert!(!vars.contains_key("FILES_DIR"));
        assert!(!vars.contains_key("NPROC"));
        assert!(!vars.contains_key("version"));
        assert!(!vars.contains_key("CFLAGS"));
        assert!(!vars.contains_key("CXXFLAGS"));
    }
}

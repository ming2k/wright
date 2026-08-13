use std::collections::HashMap;
use std::fmt::Write as _;

use crate::error::{Result, WrightError};
use crate::foundry::checkpoint::Checkpoint;
use crate::foundry::executor::{self, ExecutorRegistry};
use crate::isolation::IsolationLevel;
use wright_plan::manifest::{PipelineStage, PlanManifest};

use super::STAGES;

pub fn stage_order_for_manifest(manifest: &PlanManifest, build_phase: Option<&str>) -> Vec<String> {
    if build_phase == Some("mvp")
        && let Some(ref cfg) = manifest.mvp
        && let Some(ref order) = cfg.pipeline_order
    {
        return order.stages.clone();
    }
    if let Some(ref order) = manifest.pipeline_order {
        return order.stages.clone();
    }
    STAGES.iter().map(|s| s.to_string()).collect()
}

pub(super) fn manifest_stage<'a>(
    manifest: &'a PlanManifest,
    build_phase: Option<&str>,
    name: &str,
) -> Option<&'a PipelineStage> {
    if build_phase == Some("mvp")
        && let Some(stage) = manifest
            .mvp
            .as_ref()
            .and_then(|config| config.pipeline.get(name))
    {
        return Some(stage);
    }
    manifest.pipeline.get(name)
}

fn stage_with_hooks(stage_name: &str) -> [String; 3] {
    [
        format!("pre_{stage_name}"),
        stage_name.to_string(),
        format!("post_{stage_name}"),
    ]
}

pub(super) fn executor_for_stage<'a>(
    stage_name: &str,
    stage: &PipelineStage,
    executors: &'a ExecutorRegistry,
) -> Result<&'a executor::ExecutorConfig> {
    executors.get(&stage.executor).ok_or_else(|| {
        WrightError::ForgeError(format!(
            "stage '{stage_name}' references unknown executor '{}'",
            stage.executor
        ))
    })
}

fn append_fingerprint_field(output: &mut String, name: &str, value: &str) {
    // Length-prefix values so arbitrary scripts, arguments, and environment
    // strings cannot create ambiguous checkpoint fingerprints.
    let _ = writeln!(output, "{name}:{}:{value}", value.len());
}

fn stage_checkpoint_input(
    stage_name: &str,
    stage: &PipelineStage,
    executor: &executor::ExecutorConfig,
    isolation: IsolationLevel,
) -> String {
    let mut input = String::new();
    append_fingerprint_field(&mut input, "format", "wright-stage-v2");
    append_fingerprint_field(&mut input, "stage", stage_name);
    append_fingerprint_field(&mut input, "script", &stage.script);
    append_fingerprint_field(&mut input, "isolation", &isolation.to_string());
    append_fingerprint_field(&mut input, "executor.name", &executor.name);
    append_fingerprint_field(&mut input, "executor.command", &executor.command);
    append_fingerprint_field(&mut input, "executor.delivery", &executor.delivery);
    append_fingerprint_field(
        &mut input,
        "executor.tempfile_extension",
        &executor.tempfile_extension,
    );
    for (index, argument) in executor.args.iter().enumerate() {
        append_fingerprint_field(&mut input, &format!("executor.arg.{index}"), argument);
    }
    for (index, path) in executor.required_paths.iter().enumerate() {
        append_fingerprint_field(&mut input, &format!("executor.required_path.{index}"), path);
    }
    for (name, value) in stage
        .env
        .iter()
        .collect::<std::collections::BTreeMap<_, _>>()
    {
        append_fingerprint_field(&mut input, &format!("stage.env.{name}"), value);
    }
    input
}

pub(crate) fn compute_expected_hashes(
    manifest: &PlanManifest,
    stage_order: &[String],
    env: &HashMap<String, String>,
    build_phase: Option<&str>,
    executors: &ExecutorRegistry,
    global_default: IsolationLevel,
) -> Result<HashMap<String, String>> {
    let mut results = HashMap::new();
    let mut prev_hash = String::new();

    for name in stage_order {
        let mut ordered_input = String::new();
        for effective_name in stage_with_hooks(name) {
            let Some(stage) = manifest_stage(manifest, build_phase, &effective_name) else {
                continue;
            };
            let executor = executor_for_stage(&effective_name, stage, executors)?;
            let isolation = executor::resolve_isolation(
                &effective_name,
                stage.isolation.as_deref(),
                executor,
                global_default,
            )?;
            ordered_input.push_str(&stage_checkpoint_input(
                &effective_name,
                stage,
                executor,
                isolation,
            ));
        }
        if !ordered_input.is_empty() {
            let h = Checkpoint::compute_input_hash(&ordered_input, env, &prev_hash);
            results.insert(name.clone(), h.clone());
            prev_hash = h;
        }
    }
    Ok(results)
}

pub(crate) fn effective_manifest_isolation(
    manifest: &PlanManifest,
    build_phase: Option<&str>,
    executors: &ExecutorRegistry,
    global_default: IsolationLevel,
) -> Result<IsolationLevel> {
    let mut weakest = None;
    for name in stage_order_for_manifest(manifest, build_phase) {
        for effective_name in stage_with_hooks(&name) {
            let Some(stage) = manifest_stage(manifest, build_phase, &effective_name) else {
                continue;
            };
            let executor = executor_for_stage(&effective_name, stage, executors)?;
            let level = executor::resolve_isolation(
                &effective_name,
                stage.isolation.as_deref(),
                executor,
                global_default,
            )?;
            weakest = Some(weakest.map_or(level, |current: IsolationLevel| current.min(level)));
        }
    }
    Ok(weakest.unwrap_or(global_default))
}

/// Resolve the weakest isolation level across one canonical stage and its
/// pre/post hooks.  The forge uses this to pick the layering mode for the
/// whole stage: namespace-isolated stages run on a sandbox-mounted overlay,
/// while a stage whose weakest hook runs unisolated falls back to a real
/// populated working tree for every hook.
pub(super) fn effective_stage_isolation(
    manifest: &PlanManifest,
    build_phase: Option<&str>,
    stage_name: &str,
    executors: &ExecutorRegistry,
    global_default: IsolationLevel,
) -> Result<IsolationLevel> {
    let mut weakest = None;
    for effective_name in stage_with_hooks(stage_name) {
        let Some(stage) = manifest_stage(manifest, build_phase, &effective_name) else {
            continue;
        };
        let executor = executor_for_stage(&effective_name, stage, executors)?;
        let level = executor::resolve_isolation(
            &effective_name,
            stage.isolation.as_deref(),
            executor,
            global_default,
        )?;
        weakest = Some(weakest.map_or(level, |current: IsolationLevel| current.min(level)));
    }
    Ok(weakest.unwrap_or(global_default))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inherited_manifest() -> PlanManifest {
        PlanManifest::parse(
            r#"
name = "isolation-default-test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[pipeline.pre_compile]
script = "echo preparing"

[pipeline.compile]
script = "echo compiling"
"#,
        )
        .unwrap()
    }

    #[test]
    fn inherited_stage_uses_global_isolation() {
        let manifest = inherited_manifest();
        let executors = ExecutorRegistry::new();

        assert_eq!(
            effective_manifest_isolation(&manifest, None, &executors, IsolationLevel::None)
                .unwrap(),
            IsolationLevel::None
        );
    }

    #[test]
    fn checkpoint_hash_includes_effective_isolation() {
        let manifest = inherited_manifest();
        let executors = ExecutorRegistry::new();
        let order = stage_order_for_manifest(&manifest, None);
        let env = HashMap::new();

        let unisolated = compute_expected_hashes(
            &manifest,
            &order,
            &env,
            None,
            &executors,
            IsolationLevel::None,
        )
        .unwrap();
        let strict = compute_expected_hashes(
            &manifest,
            &order,
            &env,
            None,
            &executors,
            IsolationLevel::Strict,
        )
        .unwrap();

        assert_ne!(unisolated["compile"], strict["compile"]);
    }

    #[test]
    fn checkpoint_hash_includes_hook_configuration() {
        let mut manifest = inherited_manifest();
        let executors = ExecutorRegistry::new();
        let order = stage_order_for_manifest(&manifest, None);
        let env = HashMap::new();
        let before = compute_expected_hashes(
            &manifest,
            &order,
            &env,
            None,
            &executors,
            IsolationLevel::Strict,
        )
        .unwrap();

        manifest.pipeline.get_mut("pre_compile").unwrap().script = "echo changed".to_string();
        let after = compute_expected_hashes(
            &manifest,
            &order,
            &env,
            None,
            &executors,
            IsolationLevel::Strict,
        )
        .unwrap();

        assert_ne!(before["compile"], after["compile"]);
    }
}

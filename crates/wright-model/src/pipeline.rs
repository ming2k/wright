/// Built-in forge stages used when a plan does not declare a custom order.
pub const DEFAULT_PIPELINE_STAGES: &[&str] =
    &["prepare", "configure", "compile", "check", "staging"];

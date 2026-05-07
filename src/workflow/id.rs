use std::fmt::{self, Write as _};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::errors::Result;

const WORKFLOW_PREFIX: &[u8] = b"wright-workflow:v1\n";
const STEP_PREFIX: &[u8] = b"wright-step:v1\n";

/// Content-addressed identity for a workflow.
///
/// Two invocations of the same command with the same canonical inputs produce
/// the same `WorkflowId`. This is what makes resume work without explicit flags.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowId(String);

/// Content-addressed identity for a step within a workflow.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepId(String);

impl WorkflowId {
    pub fn derive(kind: &str, canonical_inputs: &str) -> Self {
        let mut h = Sha256::new();
        h.update(WORKFLOW_PREFIX);
        h.update(kind.as_bytes());
        h.update([0u8]);
        h.update(canonical_inputs.as_bytes());
        Self(format!("{:x}", h.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(12)]
    }
}

impl fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl StepId {
    pub fn derive(workflow: &WorkflowId, kind: &str, canonical_inputs: &str) -> Self {
        let mut h = Sha256::new();
        h.update(STEP_PREFIX);
        h.update(workflow.0.as_bytes());
        h.update([0u8]);
        h.update(kind.as_bytes());
        h.update([0u8]);
        h.update(canonical_inputs.as_bytes());
        Self(format!("{:x}", h.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(12)]
    }
}

impl fmt::Display for StepId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for WorkflowId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<String> for StepId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Deterministic JSON serialization: object keys sorted, no insignificant
/// whitespace, numbers formatted by `serde_json` (which already produces a
/// stable form for finite values).
///
/// This is the only allowed source of bytes for hashing inputs. CLI args,
/// step inputs, and any other hash material must round-trip through here.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<String> {
    let v = serde_json::to_value(value)?;
    let mut out = String::new();
    write_canonical(&v, &mut out);
    Ok(out)
}

fn write_canonical(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value::*;
    match v {
        Null => out.push_str("null"),
        Bool(b) => write!(out, "{}", b).expect("string write"),
        Number(n) => write!(out, "{}", n).expect("string write"),
        String(s) => {
            // serde_json already produces a properly escaped JSON string literal.
            out.push_str(&serde_json::to_string(s).expect("string serialization"));
        }
        Array(arr) => {
            out.push('[');
            for (i, x) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out);
            }
            out.push(']');
        }
        Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(|s| s.as_str()).collect();
            keys.sort_unstable();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("key serialization"));
                out.push(':');
                write_canonical(&map[*k], out);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct A {
        b: u32,
        a: u32,
    }

    #[derive(Serialize)]
    struct B {
        a: u32,
        b: u32,
    }

    #[test]
    fn canonical_json_sorts_keys_regardless_of_struct_order() {
        let a = canonical_json(&A { b: 2, a: 1 }).unwrap();
        let b = canonical_json(&B { a: 1, b: 2 }).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn workflow_id_is_deterministic() {
        let inputs = canonical_json(&serde_json::json!({"target": "foo"})).unwrap();
        let a = WorkflowId::derive("build", &inputs);
        let b = WorkflowId::derive("build", &inputs);
        assert_eq!(a, b);
    }

    #[test]
    fn workflow_id_differs_by_kind() {
        let inputs = canonical_json(&serde_json::json!({"target": "foo"})).unwrap();
        let a = WorkflowId::derive("build", &inputs);
        let b = WorkflowId::derive("apply", &inputs);
        assert_ne!(a, b);
    }

    #[test]
    fn step_id_differs_by_workflow() {
        let wf_a = WorkflowId::derive("build", "{}");
        let wf_b = WorkflowId::derive("apply", "{}");
        let inputs = "{}";
        assert_ne!(
            StepId::derive(&wf_a, "resolve", inputs),
            StepId::derive(&wf_b, "resolve", inputs),
        );
    }

    #[test]
    fn step_id_differs_by_kind() {
        let wf = WorkflowId::derive("build", "{}");
        assert_ne!(
            StepId::derive(&wf, "resolve", "{}"),
            StepId::derive(&wf, "build_plan", "{}"),
        );
    }
}

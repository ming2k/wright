//! Universal plan/output identifier for deployed targets.
//!
//! User-facing commands that address deployed things share one identifier
//! grammar with two addressing schemes:
//!
//! - **plan-level** — `plan` or `plan:*` addresses every deployed output of
//!   the named plan.
//! - **output-level** — `output` or `plan:output` addresses a single
//!   deployed output. The `plan:output` form is absolute: the output must
//!   actually belong to the named plan.
//!
//! A bare name is resolved against both the plan registry and the deployed
//! outputs. When both match and they do not coincide (a single-output plan
//! whose output carries the plan name), the identifier is ambiguous and
//! resolution fails, naming the absolute forms the user can write instead.

use crate::error::{Result, WrightError};
use wright_state::database::{InstalledDb, PartWithPlan, PlanRecord};

/// A parsed target identifier — pure syntax, no database access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identifier {
    /// Bare name (`llvm`); plan-level or output-level is decided at
    /// resolve time.
    Bare(String),
    /// Absolute plan-level reference (`llvm:*`).
    Plan(String),
    /// Absolute output-level reference (`llvm:clang`).
    Output { plan: String, output: String },
}

impl Identifier {
    /// Parse a user-supplied target into an [`Identifier`].
    ///
    /// Accepted forms: `plan`, `output`, `plan:*`, `plan:output`. Anything
    /// with empty components or more than one `:` is rejected.
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.is_empty() {
            return Err(WrightError::ValidationError(
                "empty target identifier".to_string(),
            ));
        }

        match input.split_once(':') {
            None => Ok(Identifier::Bare(input.to_string())),
            Some((plan, selector)) => {
                let plan = plan.trim();
                let selector = selector.trim();
                if plan.is_empty() || selector.is_empty() {
                    return Err(WrightError::ValidationError(format!(
                        "invalid target '{}': plan and output names must be non-empty",
                        input
                    )));
                }
                if selector.contains(':') {
                    return Err(WrightError::ValidationError(format!(
                        "invalid target '{}': expected `plan`, `plan:*`, or `plan:output`",
                        input
                    )));
                }
                if selector == "*" {
                    Ok(Identifier::Plan(plan.to_string()))
                } else {
                    Ok(Identifier::Output {
                        plan: plan.to_string(),
                        output: selector.to_string(),
                    })
                }
            }
        }
    }
}

/// An identifier resolved against the installed-state database.
#[derive(Debug)]
pub enum ResolvedTarget {
    /// Plan-level: every deployed output of the plan, ordered by name.
    Plan {
        plan: PlanRecord,
        parts: Vec<PartWithPlan>,
    },
    /// Output-level: exactly one deployed output.
    Output { part: PartWithPlan },
}

impl ResolvedTarget {
    /// Every deployed part this target addresses, ordered by name.
    pub fn parts(&self) -> &[PartWithPlan] {
        match self {
            ResolvedTarget::Plan { parts, .. } => parts,
            ResolvedTarget::Output { part } => std::slice::from_ref(part),
        }
    }
}

/// Resolve an [`Identifier`] against the installed-state database.
pub async fn resolve(db: &InstalledDb, ident: &Identifier) -> Result<ResolvedTarget> {
    match ident {
        Identifier::Bare(name) => resolve_bare(db, name).await,
        Identifier::Plan(plan) => resolve_plan(db, plan).await,
        Identifier::Output { plan, output } => resolve_output(db, plan, output).await,
    }
}

/// Resolve a bare name: plan registry first, then deployed outputs, with
/// collision detection between the two schemes.
async fn resolve_bare(db: &InstalledDb, name: &str) -> Result<ResolvedTarget> {
    let plan = db
        .get_plan(name)
        .await
        .map_err(|e| WrightError::context("failed to query plan", e))?;
    let part = db
        .get_part_with_plan(name)
        .await
        .map_err(|e| WrightError::context("failed to query part", e))?;

    match (plan, part) {
        (Some(plan), Some(part)) => {
            let plan_parts = plan_parts(db, &plan).await?;
            if plan_parts.is_empty() || (part.plan_id == plan.id && plan_parts.len() == 1) {
                // The plan shell has no deployed outputs, or plan and output
                // coincide (single-output plan whose output carries the plan
                // name): one concrete target, no ambiguity.
                Ok(ResolvedTarget::Output { part })
            } else {
                Err(WrightError::AmbiguousTarget(format!(
                    "'{}' matches both plan '{}' ({} deployed outputs) and output '{}' of plan '{}'; \
                     use '{}:*' for the whole plan or '{}:{}' for just the output",
                    name,
                    plan.name,
                    plan_parts.len(),
                    part.name,
                    part.plan_name,
                    name,
                    part.plan_name,
                    part.name,
                )))
            }
        }
        (Some(plan), None) => resolve_plan_parts(db, plan).await,
        (None, Some(part)) => Ok(ResolvedTarget::Output { part }),
        (None, None) => Err(WrightError::PartNotFound(format!(
            "no deployed plan or output named '{}'",
            name
        ))),
    }
}

/// Resolve an absolute plan-level reference (`plan:*`).
async fn resolve_plan(db: &InstalledDb, name: &str) -> Result<ResolvedTarget> {
    let plan = db
        .get_plan(name)
        .await
        .map_err(|e| WrightError::context("failed to query plan", e))?
        .ok_or_else(|| WrightError::PartNotFound(format!("plan '{}' is not deployed", name)))?;
    resolve_plan_parts(db, plan).await
}

/// Resolve an absolute output-level reference (`plan:output`), validating
/// that the output actually belongs to the named plan.
async fn resolve_output(db: &InstalledDb, plan: &str, output: &str) -> Result<ResolvedTarget> {
    let plan_record = db
        .get_plan(plan)
        .await
        .map_err(|e| WrightError::context("failed to query plan", e))?
        .ok_or_else(|| WrightError::PartNotFound(format!("plan '{}' is not deployed", plan)))?;
    let part = db
        .get_part_with_plan(output)
        .await
        .map_err(|e| WrightError::context("failed to query part", e))?
        .ok_or_else(|| WrightError::PartNotFound(format!("output '{}' is not deployed", output)))?;

    if part.plan_id != plan_record.id {
        return Err(WrightError::ValidationError(format!(
            "output '{}' is not an output of plan '{}' (it belongs to plan '{}'); use '{}:{}'",
            output, plan, part.plan_name, part.plan_name, output
        )));
    }
    Ok(ResolvedTarget::Output { part })
}

/// Fetch the deployed outputs of a plan record.
async fn plan_parts(db: &InstalledDb, plan: &PlanRecord) -> Result<Vec<PartWithPlan>> {
    db.get_parts_by_plan(&plan.name)
        .await
        .map_err(|e| WrightError::context("failed to query plan outputs", e))
}

/// Shared tail for plan-level resolution: a plan without deployed outputs
/// is not a usable target.
async fn resolve_plan_parts(db: &InstalledDb, plan: PlanRecord) -> Result<ResolvedTarget> {
    let parts = plan_parts(db, &plan).await?;
    if parts.is_empty() {
        return Err(WrightError::PartNotFound(format!(
            "plan '{}' has no deployed outputs",
            plan.name
        )));
    }
    Ok(ResolvedTarget::Plan { plan, parts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::{NewPart, NewPlan};

    async fn test_db() -> InstalledDb {
        InstalledDb::open_in_memory().await.unwrap()
    }

    async fn add_plan(db: &InstalledDb, name: &str, outputs: &[&str]) {
        let plan_id = db
            .insert_plan(NewPlan {
                name,
                ..Default::default()
            })
            .await
            .unwrap();
        for output in outputs {
            db.insert_part(NewPart {
                name: output,
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();
        }
    }

    async fn resolve_str(db: &InstalledDb, input: &str) -> Result<ResolvedTarget> {
        resolve(db, &Identifier::parse(input).unwrap()).await
    }

    #[test]
    fn parse_accepts_the_documented_forms() {
        assert_eq!(
            Identifier::parse("llvm").unwrap(),
            Identifier::Bare("llvm".to_string())
        );
        assert_eq!(
            Identifier::parse("llvm:*").unwrap(),
            Identifier::Plan("llvm".to_string())
        );
        assert_eq!(
            Identifier::parse("llvm:clang").unwrap(),
            Identifier::Output {
                plan: "llvm".to_string(),
                output: "clang".to_string()
            }
        );
        assert_eq!(
            Identifier::parse("  llvm : clang  ").unwrap(),
            Identifier::Output {
                plan: "llvm".to_string(),
                output: "clang".to_string()
            }
        );
    }

    #[test]
    fn parse_rejects_malformed_forms() {
        for bad in ["", "  ", "llvm:", ":clang", "llvm:clang:extra"] {
            assert!(
                Identifier::parse(bad).is_err(),
                "expected parse error for {:?}",
                bad
            );
        }
    }

    #[tokio::test]
    async fn bare_output_resolves_to_single_output() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang"]).await;

        let resolved = resolve_str(&db, "clang").await.unwrap();
        let ResolvedTarget::Output { part } = resolved else {
            panic!("expected output-level resolution");
        };
        assert_eq!(part.name, "clang");
        assert_eq!(part.plan_name, "llvm");
    }

    #[tokio::test]
    async fn bare_plan_resolves_to_every_output() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;

        let resolved = resolve_str(&db, "llvm").await.unwrap();
        let ResolvedTarget::Plan { plan, parts } = resolved else {
            panic!("expected plan-level resolution");
        };
        assert_eq!(plan.name, "llvm");
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["clang", "lld"]);
    }

    #[tokio::test]
    async fn bare_single_output_plan_coincides_without_ambiguity() {
        let db = test_db().await;
        add_plan(&db, "zlib", &["zlib"]).await;

        let resolved = resolve_str(&db, "zlib").await.unwrap();
        assert_eq!(resolved.parts().len(), 1);
        assert_eq!(resolved.parts()[0].name, "zlib");
    }

    #[tokio::test]
    async fn bare_name_shared_with_multi_output_plan_is_ambiguous() {
        let db = test_db().await;
        add_plan(&db, "gcc", &["gcc", "libstdc++"]).await;

        let err = resolve_str(&db, "gcc").await.unwrap_err();
        let WrightError::AmbiguousTarget(msg) = err else {
            panic!("expected ambiguous target, got: {}", err);
        };
        assert!(msg.contains("gcc:*"), "message names plan form: {}", msg);
        assert!(
            msg.contains("gcc:gcc"),
            "message names output form: {}",
            msg
        );
    }

    #[tokio::test]
    async fn bare_name_shared_with_other_plans_output_is_ambiguous() {
        let db = test_db().await;
        // Plan `a` deploys an output named `x`; an unrelated plan `x` also
        // exists with its own output.
        add_plan(&db, "a", &["x"]).await;
        add_plan(&db, "x", &["y"]).await;

        let err = resolve_str(&db, "x").await.unwrap_err();
        let WrightError::AmbiguousTarget(msg) = err else {
            panic!("expected ambiguous target, got: {}", err);
        };
        assert!(msg.contains("x:*"), "message names plan form: {}", msg);
        assert!(msg.contains("a:x"), "message names output form: {}", msg);
    }

    #[tokio::test]
    async fn wildcard_resolves_whole_plan() {
        let db = test_db().await;
        add_plan(&db, "gcc", &["gcc", "libstdc++"]).await;

        let resolved = resolve_str(&db, "gcc:*").await.unwrap();
        let ResolvedTarget::Plan { parts, .. } = resolved else {
            panic!("expected plan-level resolution");
        };
        assert_eq!(parts.len(), 2);
    }

    #[tokio::test]
    async fn wildcard_on_unknown_plan_is_not_found() {
        let db = test_db().await;
        let err = resolve_str(&db, "nope:*").await.unwrap_err();
        assert!(matches!(err, WrightError::PartNotFound(_)));
    }

    #[tokio::test]
    async fn qualified_output_resolves_and_validates_membership() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_plan(&db, "gcc", &["gcc"]).await;

        let resolved = resolve_str(&db, "llvm:clang").await.unwrap();
        let ResolvedTarget::Output { part } = resolved else {
            panic!("expected output-level resolution");
        };
        assert_eq!(part.name, "clang");

        let err = resolve_str(&db, "gcc:clang").await.unwrap_err();
        let WrightError::ValidationError(msg) = err else {
            panic!("expected validation error, got: {}", err);
        };
        assert!(msg.contains("llvm:clang"), "message hints truth: {}", msg);

        let err = resolve_str(&db, "llvm:nope").await.unwrap_err();
        assert!(matches!(err, WrightError::PartNotFound(_)));

        let err = resolve_str(&db, "nope:clang").await.unwrap_err();
        assert!(matches!(err, WrightError::PartNotFound(_)));
    }

    #[tokio::test]
    async fn bare_unknown_name_is_not_found() {
        let db = test_db().await;
        let err = resolve_str(&db, "nope").await.unwrap_err();
        let WrightError::PartNotFound(msg) = err else {
            panic!("expected part not found, got: {}", err);
        };
        assert!(msg.contains("nope"));
    }
}

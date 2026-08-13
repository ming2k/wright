use crate::error::Result;
use wright_state::database::{InstalledDb, Origin, PartWithPlan};

#[allow(clippy::too_many_arguments)]
pub async fn execute_list(
    db: &InstalledDb,
    long: bool,
    roots: bool,
    provided: bool,
    orphans: bool,
    json: bool,
    plans: bool,
    parts_only: bool,
) -> Result<()> {
    let parts = if provided {
        db.get_provided_parts().await?
    } else if orphans {
        db.get_orphan_parts().await?
    } else if roots {
        db.get_root_parts().await?
    } else {
        db.list_parts().await?
    };

    if json {
        if plans {
            let out: Vec<serde_json::Value> = unique_plans(&parts)
                .iter()
                .map(|plan| {
                    serde_json::json!({
                        "name": plan.name,
                        "version": plan.version,
                        "release": plan.release,
                        "epoch": plan.epoch,
                        "arch": plan.arch,
                    })
                })
                .collect();
            return super::print_json(&out);
        }
        let out: Vec<serde_json::Value> = parts
            .iter()
            .map(|part| {
                serde_json::json!({
                    "name": part.name.as_str(),
                    "version": part.version.as_str(),
                    "release": part.release,
                    "epoch": part.epoch,
                    "arch": part.arch.as_str(),
                    "origin": part.origin.to_string(),
                    "plan_name": part.plan_name.as_str(),
                })
            })
            .collect();
        return super::print_json(&out);
    }

    if parts.is_empty() {
        if !provided && !roots && !orphans {
            if plans {
                println!("no plans deployed");
            } else {
                println!("no parts deployed");
            }
        }
        return Ok(());
    }

    let lines = if plans {
        format_plans(&unique_plans(&parts), long)
    } else if parts_only {
        format_parts(&parts, long)
    } else {
        format_grouped(&parts, long)
    };
    for line in lines {
        println!("{}", line);
    }
    Ok(())
}

/// Plan identity plus its version tuple, read off a part row (the version
/// columns live on the plan row since schema V8).
struct PlanSummary<'a> {
    name: &'a str,
    version: &'a str,
    release: i64,
    epoch: i64,
    arch: &'a str,
}

/// One entry per plan, ordered by plan name. A plan is dropped from the
/// registry when its last part is removed, so the part list is the source
/// of truth for which plans are deployed.
fn unique_plans<'a>(parts: &'a [PartWithPlan]) -> Vec<PlanSummary<'a>> {
    let mut plans: Vec<PlanSummary<'a>> = Vec::new();
    for part in parts {
        if plans.iter().any(|plan| plan.name == part.plan_name) {
            continue;
        }
        plans.push(PlanSummary {
            name: part.plan_name.as_str(),
            version: part.version.as_str(),
            release: part.release,
            epoch: part.epoch,
            arch: part.arch.as_str(),
        });
    }
    plans.sort_by(|a, b| a.name.cmp(b.name));
    plans
}

fn ver_rel_arch(version: &str, release: i64, arch: &str) -> String {
    if version.is_empty() {
        format!("{}-{}", release, arch)
    } else {
        format!("{}-{}-{}", version, release, arch)
    }
}

/// Default view: plans with their parts indented beneath them, mirroring
/// the plan/output targets accepted by commands like `remove` and `files`.
fn format_grouped(parts: &[PartWithPlan], long: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for plan in unique_plans(parts) {
        lines.push(plan.name.to_string());
        for part in parts.iter().filter(|part| part.plan_name == plan.name) {
            if !long {
                lines.push(format!("  {}", part.name));
            } else if part.origin == Origin::External {
                let ver = if part.version.is_empty() {
                    "-"
                } else {
                    &part.version
                };
                lines.push(format!("  {:<12} {:<24} {}", "external", part.name, ver));
            } else {
                lines.push(format!(
                    "  {:<12} {:<24} {:<20}",
                    part.origin,
                    part.name,
                    ver_rel_arch(&part.version, part.release, &part.arch)
                ));
            }
        }
    }
    lines
}

/// Flat plan names, one per line (pipe-friendly).
fn format_plans(plans: &[PlanSummary<'_>], long: bool) -> Vec<String> {
    plans
        .iter()
        .map(|plan| {
            if long {
                format!(
                    "{:<24} {}",
                    plan.name,
                    ver_rel_arch(plan.version, plan.release, plan.arch)
                )
            } else {
                plan.name.to_string()
            }
        })
        .collect()
}

/// Flat part names, one per line (pipe-friendly).
fn format_parts(parts: &[PartWithPlan], long: bool) -> Vec<String> {
    parts
        .iter()
        .map(|part| {
            if !long {
                return part.name.clone();
            }
            let ver = if part.version.is_empty() {
                "-"
            } else {
                &part.version
            };
            if part.origin == Origin::External {
                format!("{:<12} {:<24} {}", "external", part.name, ver)
            } else {
                let plan_info = format!("{} {}", part.plan_name, part.version);
                format!(
                    "{:<12} {:<24} {:<20} {}",
                    part.origin,
                    part.name,
                    ver_rel_arch(&part.version, part.release, &part.arch),
                    plan_info
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::{NewPart, NewPlan};

    async fn test_db() -> InstalledDb {
        InstalledDb::open_in_memory().await.unwrap()
    }

    async fn add_plan(db: &InstalledDb, name: &str, version: &str, outputs: &[&str]) {
        let plan_id = db
            .insert_plan(NewPlan {
                name,
                version,
                release: 1,
                arch: "x86_64",
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

    #[tokio::test]
    async fn grouped_view_lists_each_plan_with_its_parts() {
        let db = test_db().await;
        add_plan(&db, "llvm", "20.1.0", &["clang", "lld"]).await;
        add_plan(&db, "zlib", "1.3.1", &["zlib"]).await;

        let parts = db.list_parts().await.unwrap();
        let lines = format_grouped(&parts, false);
        assert_eq!(lines, ["llvm", "  clang", "  lld", "zlib", "  zlib"]);
    }

    #[tokio::test]
    async fn grouped_long_view_adds_origin_and_version_per_part() {
        let db = test_db().await;
        add_plan(&db, "zlib", "1.3.1", &["zlib"]).await;

        let parts = db.list_parts().await.unwrap();
        let lines = format_grouped(&parts, true);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "zlib");
        assert!(lines[1].starts_with("  manual"));
        assert!(lines[1].contains("zlib"));
        assert!(lines[1].contains("1.3.1-1-x86_64"));
    }

    #[tokio::test]
    async fn plans_view_lists_each_plan_once() {
        let db = test_db().await;
        add_plan(&db, "llvm", "20.1.0", &["clang", "lld"]).await;
        add_plan(&db, "zlib", "1.3.1", &["zlib"]).await;

        let parts = db.list_parts().await.unwrap();
        let lines = format_plans(&unique_plans(&parts), false);
        assert_eq!(lines, ["llvm", "zlib"]);

        let long_lines = format_plans(&unique_plans(&parts), true);
        assert!(long_lines[0].contains("llvm"));
        assert!(long_lines[0].contains("20.1.0-1-x86_64"));
    }

    #[tokio::test]
    async fn parts_view_keeps_flat_one_per_line_output() {
        let db = test_db().await;
        add_plan(&db, "llvm", "20.1.0", &["clang", "lld"]).await;
        add_plan(&db, "zlib", "1.3.1", &["zlib"]).await;

        let parts = db.list_parts().await.unwrap();
        let lines = format_parts(&parts, false);
        assert_eq!(lines, ["clang", "lld", "zlib"]);

        let long_lines = format_parts(&parts, true);
        assert!(long_lines[0].contains("clang"));
        assert!(long_lines[0].contains("llvm 20.1.0"));
    }

    #[tokio::test]
    async fn execute_list_accepts_all_views_filters_and_json() {
        let db = test_db().await;
        add_plan(&db, "llvm", "20.1.0", &["clang", "lld"]).await;

        for json in [false, true] {
            for (plans, parts_only) in [(false, false), (true, false), (false, true)] {
                execute_list(&db, false, false, false, false, json, plans, parts_only)
                    .await
                    .unwrap();
                execute_list(&db, true, false, false, false, json, plans, parts_only)
                    .await
                    .unwrap();
            }
        }
        for (roots, provided, orphans) in [(true, false, false), (false, true, true)] {
            execute_list(&db, true, roots, provided, orphans, false, false, false)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn execute_list_on_empty_database_is_quiet() {
        let db = test_db().await;
        for (plans, parts_only) in [(false, false), (true, false), (false, true)] {
            execute_list(&db, false, false, false, false, false, plans, parts_only)
                .await
                .unwrap();
        }
    }
}

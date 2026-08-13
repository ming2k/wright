use crate::error::Result;
use wright_state::database::{InstalledDb, Origin, PartWithPlan};

pub async fn execute_list(
    db: &InstalledDb,
    long: bool,
    filter: Option<&str>,
    json: bool,
    plan_only: bool,
    part_only: bool,
) -> Result<()> {
    let filter_str = filter.unwrap_or("").trim().to_lowercase();
    let parts = if filter_str == "provided" {
        db.get_provided_parts().await?
    } else if filter_str == "orphan" || filter_str == "orphans" {
        db.get_orphan_parts().await?
    } else if filter_str == "leaf" || filter_str == "roots" || filter_str == "root" {
        db.get_root_parts().await?
    } else {
        let all = db.list_parts().await?;
        if filter_str.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|p| {
                    p.name.to_lowercase().contains(&filter_str)
                        || p.plan_name.to_lowercase().contains(&filter_str)
                        || format!("{}:{}", p.plan_name, p.name).to_lowercase().contains(&filter_str)
                })
                .collect()
        }
    };

    if json {
        if plan_only {
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
                    "target": format!("{}:{}", part.plan_name, part.name),
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
        if filter.is_none() {
            if plan_only {
                println!("no plans deployed");
            } else {
                println!("no parts deployed");
            }
        }
        return Ok(());
    }

    let lines = if plan_only {
        format_plans(&unique_plans(&parts), long)
    } else if part_only {
        format_parts(&parts, long)
    } else {
        format_targets(&parts, long)
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

/// Default view: canonical target identifiers (plan:output, e.g. optics:flux).
fn format_targets(parts: &[PartWithPlan], long: bool) -> Vec<String> {
    parts
        .iter()
        .map(|part| {
            let target = format!("{}:{}", part.plan_name, part.name);
            if !long {
                return target;
            }
            if part.origin == Origin::External {
                let ver = if part.version.is_empty() {
                    "-"
                } else {
                    &part.version
                };
                format!("{:<12} {:<24} {}", "external", target, ver)
            } else {
                format!(
                    "{:<12} {:<24} {:<20}",
                    part.origin,
                    target,
                    ver_rel_arch(&part.version, part.release, &part.arch)
                )
            }
        })
        .collect()
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
    async fn default_target_view_lists_canonical_targets() {
        let db = test_db().await;
        add_plan(&db, "llvm", "20.1.0", &["clang", "lld"]).await;
        add_plan(&db, "zlib", "1.3.1", &["zlib"]).await;

        let parts = db.list_parts().await.unwrap();
        let lines = format_targets(&parts, false);
        assert_eq!(lines, ["llvm:clang", "llvm:lld", "zlib:zlib"]);
    }

    #[tokio::test]
    async fn target_long_view_adds_origin_and_version_per_target() {
        let db = test_db().await;
        add_plan(&db, "zlib", "1.3.1", &["zlib"]).await;

        let parts = db.list_parts().await.unwrap();
        let lines = format_targets(&parts, true);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("manual"));
        assert!(lines[0].contains("zlib:zlib"));
        assert!(lines[0].contains("1.3.1-1-x86_64"));
    }

    #[tokio::test]
    async fn plan_only_view_lists_each_plan_once() {
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
    async fn part_only_view_keeps_flat_one_per_line_output() {
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
            for (plan_only, part_only) in [(false, false), (true, false), (false, true)] {
                execute_list(&db, false, None, json, plan_only, part_only)
                    .await
                    .unwrap();
                execute_list(&db, true, None, json, plan_only, part_only)
                    .await
                    .unwrap();
            }
        }
        for filter in ["leaf", "provided", "orphan", "llvm"] {
            execute_list(&db, true, Some(filter), false, false, false)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn execute_list_on_empty_database_is_quiet() {
        let db = test_db().await;
        for (plan_only, part_only) in [(false, false), (true, false), (false, true)] {
            execute_list(&db, false, None, false, plan_only, part_only)
                .await
                .unwrap();
        }
    }
}


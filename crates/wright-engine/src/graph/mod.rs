//! Global plan relationship graph (`wright graph`).
//!
//! The graph is a read-only projection over two inputs: the plan index
//! (every `plan.toml` under the configured search dirs) and the installed
//! state database. Nodes are plans plus one synthetic node per dependency
//! name that no discovered plan provides (virtual names, system libraries);
//! edges are dependency relationships labelled by domain. The same document
//! backs both the terminal summary ([`render_terminal`]) and the `--web`
//! JSON endpoint (`server`), so both views always agree.

pub mod server;

use std::collections::HashSet;

use serde::Serialize;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightResultExt};
use crate::resolve::plan_search_dirs;
use wright_model::version;
use wright_plan::discovery::PlanIndex;
use wright_plan::manifest::{OutputConfig, PlanManifest};
use wright_state::database::InstalledDb;

/// Serializable graph document served at `/api/graph` and rendered by the
/// terminal view. Field order and element order are deterministic.
#[derive(Debug, Clone, Serialize)]
pub struct GraphDoc {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub name: String,
    pub version: Option<String>,
    pub release: u32,
    pub description: String,
    pub url: Option<String>,
    pub state: NodeState,
    pub outputs: Vec<String>,
    pub replaces: Vec<String>,
    pub conflicts: Vec<String>,
}

/// Deployment state of a plan relative to its manifest, using the same
/// epoch/version/release comparison as `wright resolve --match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Installed,
    Outdated,
    Missing,
    /// Synthetic node: a dependency name no discovered plan provides.
    External,
}

impl NodeState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Outdated => "outdated",
            Self::Missing => "missing",
            Self::External => "external",
        }
    }
}

/// A dependency edge: `from` depends on `to` in the given domain.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub domain: EdgeDomain,
}

/// Dependency field an edge was read from. Declaration order doubles as the
/// display order in the terminal view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDomain {
    Build,
    Link,
    Runtime,
}

impl EdgeDomain {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Link => "link",
            Self::Runtime => "runtime",
        }
    }
}

/// Build the full plan relationship graph.
///
/// Output order is deterministic: nodes sort by name, edges by
/// `(from, to, domain)`.
pub async fn build_graph(config: &GlobalConfig, db: &InstalledDb) -> Result<GraphDoc> {
    let plan_dirs = plan_search_dirs(config);
    let index = PlanIndex::discover(&plan_dirs).context("failed to discover plans")?;
    let mut manifests = index.load_all().context("failed to load plans")?;
    manifests.sort_by(|a, b| a.0.cmp(&b.0));

    let mut nodes = Vec::with_capacity(manifests.len());
    let mut edges = Vec::new();
    let mut seen_edges: HashSet<(String, String, EdgeDomain)> = HashSet::new();
    let mut external_names: Vec<String> = Vec::new();

    for (name, manifest) in &manifests {
        nodes.push(plan_node(
            name,
            manifest,
            plan_state(name, manifest, db).await?,
        ));

        let dep_fields = [
            (EdgeDomain::Build, &manifest.build_deps),
            (EdgeDomain::Link, &manifest.link_deps),
            (EdgeDomain::Runtime, &manifest.runtime_deps),
        ];
        for (domain, deps) in dep_fields {
            for dep_raw in deps {
                let target = dep_plan_name(dep_raw);
                if target.is_empty() || target == *name {
                    continue;
                }
                if index.path_for(&target).is_none() && !external_names.iter().any(|n| n == &target)
                {
                    external_names.push(target.clone());
                }
                if seen_edges.insert((name.clone(), target.clone(), domain)) {
                    edges.push(GraphEdge {
                        from: name.clone(),
                        to: target,
                        domain,
                    });
                }
            }
        }
    }

    // Synthetic nodes for dependency names outside the plan index (virtual
    // names, externally provided system libraries). They carry no metadata.
    for name in external_names {
        nodes.push(GraphNode {
            name,
            version: None,
            release: 0,
            description: String::new(),
            url: None,
            state: NodeState::External,
            outputs: Vec::new(),
            replaces: Vec::new(),
            conflicts: Vec::new(),
        });
    }

    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    edges.sort_by(|a, b| (&a.from, &a.to, a.domain).cmp(&(&b.from, &b.to, b.domain)));
    Ok(GraphDoc { nodes, edges })
}

/// Normalize a raw dependency string to the referenced plan name: strip any
/// version constraint, then reduce `plan:output` to `plan` (same pipeline as
/// the resolver).
fn dep_plan_name(dep_raw: &str) -> String {
    let dep_name = version::parse_dependency(dep_raw)
        .unwrap_or_else(|_| (dep_raw.to_string(), None))
        .0;
    version::parse_dep_ref(&dep_name).plan().to_string()
}

/// Installed/outdated/missing verdict for one plan. The comparison mirrors
/// `resolve::graph::dependency_plan_differs`: epoch, version (absent
/// manifest version reads as ""), and release must all match the registered
/// record.
async fn plan_state(name: &str, manifest: &PlanManifest, db: &InstalledDb) -> Result<NodeState> {
    let Some(record) = db
        .get_plan(name)
        .await
        .context("failed to query installed plan")?
    else {
        return Ok(NodeState::Missing);
    };
    let manifest_ver = manifest.metadata.version.as_deref().unwrap_or("");
    if record.epoch != i64::from(manifest.metadata.epoch)
        || record.version != manifest_ver
        || record.release != i64::from(manifest.metadata.release)
    {
        return Ok(NodeState::Outdated);
    }
    Ok(NodeState::Installed)
}

fn plan_node(name: &str, manifest: &PlanManifest, state: NodeState) -> GraphNode {
    let outputs = match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => parts.iter().map(|(n, _)| n.clone()).collect(),
        _ => vec![name.to_string()],
    };
    GraphNode {
        name: name.to_string(),
        version: manifest.metadata.version.clone(),
        release: manifest.metadata.release,
        description: manifest.metadata.description.clone(),
        url: manifest.metadata.url.clone(),
        state,
        outputs,
        replaces: manifest.relations.replaces.clone(),
        conflicts: manifest.relations.conflicts.clone(),
    }
}

/// Plain-text summary: one line per plan (`name version [state]`), its
/// dependency edges grouped by domain underneath, and a trailing statistics
/// line. External nodes are counted but not listed.
pub fn render_terminal(doc: &GraphDoc) {
    let mut installed = 0usize;
    let mut outdated = 0usize;
    let mut missing = 0usize;
    let mut plans = 0usize;

    for node in &doc.nodes {
        if node.state == NodeState::External {
            continue;
        }
        plans += 1;
        match node.state {
            NodeState::Installed => installed += 1,
            NodeState::Outdated => outdated += 1,
            NodeState::Missing => missing += 1,
            NodeState::External => unreachable!(),
        }
        let version = node.version.as_deref().unwrap_or("-");
        crate::outln!("{} {} [{}]", node.name, version, node.state.as_str());

        let mut deps: Vec<&GraphEdge> = doc.edges.iter().filter(|e| e.from == node.name).collect();
        deps.sort_by(|a, b| (a.domain, &a.to).cmp(&(b.domain, &b.to)));
        for (i, edge) in deps.iter().enumerate() {
            let branch = if i + 1 == deps.len() {
                "└─"
            } else {
                "├─"
            };
            crate::outln!("  {} {}: {}", branch, edge.domain.as_str(), edge.to);
        }
    }

    let external = doc
        .nodes
        .iter()
        .filter(|n| n.state == NodeState::External)
        .count();
    crate::outln!(
        "{} plans ({} installed, {} outdated, {} missing), {} edges, {} external refs",
        plans,
        installed,
        outdated,
        missing,
        doc.edges.len(),
        external
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::NewPlan;

    fn write_plan(plans_dir: &std::path::Path, name: &str, extra: &str) {
        let plan_dir = plans_dir.join(name);
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(
            plan_dir.join("plan.toml"),
            format!(
                "name = \"{name}\"\nversion = \"1.0.0\"\nrelease = 1\ndescription = \"d\"\nlicense = \"MIT\"\narch = \"x86_64\"\n{extra}"
            ),
        )
        .unwrap();
    }

    async fn fixture() -> (tempfile::TempDir, GraphDoc) {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        // app: build dep on toolchain, link dep on libfoo, runtime deps on a
        // plan:output reference and a name outside the index (external).
        // "libfoo:core" and "libfoo" normalize to the same plan, exercising
        // (from, to, domain) edge dedup. Runtime deps live under [[output]].
        write_plan(
            &plans_dir,
            "app",
            "build_deps = [\"toolchain\"]\nlink_deps = [\"libfoo\"]\n\n[[output]]\nname = \"app\"\nruntime_deps = [\"libfoo:core\", \"libfoo\", \"ghost-sys >= 1.0\"]\n",
        );
        // libfoo: multi-output plan, plus a self-dependency that must be dropped.
        write_plan(
            &plans_dir,
            "libfoo",
            "[[output]]\nname = \"core\"\ndescription = \"d\"\ninclude = [\"/usr/lib/**\"]\nruntime_deps = [\"libfoo\"]\n",
        );
        write_plan(&plans_dir, "toolchain", "");

        let db = InstalledDb::open_in_memory().await.unwrap();
        // Matches the manifest -> installed.
        db.insert_plan(NewPlan {
            name: "app",
            version: "1.0.0",
            release: 1,
            arch: "x86_64",
            ..Default::default()
        })
        .await
        .unwrap();
        // Diverges from the manifest version -> outdated.
        db.insert_plan(NewPlan {
            name: "libfoo",
            version: "0.9.0",
            release: 1,
            arch: "x86_64",
            ..Default::default()
        })
        .await
        .unwrap();
        // toolchain has no record -> missing.

        let mut config = GlobalConfig::default();
        config.general.plans_dir = plans_dir;
        config.general.extra_plans_dirs = Vec::new();
        let doc = build_graph(&config, &db).await.unwrap();
        (temp, doc)
    }

    fn node<'a>(doc: &'a GraphDoc, name: &str) -> &'a GraphNode {
        doc.nodes.iter().find(|n| n.name == name).unwrap()
    }

    #[tokio::test]
    async fn node_states_reflect_db_records() {
        let (_temp, doc) = fixture().await;
        assert_eq!(node(&doc, "app").state, NodeState::Installed);
        assert_eq!(node(&doc, "libfoo").state, NodeState::Outdated);
        assert_eq!(node(&doc, "toolchain").state, NodeState::Missing);
        assert_eq!(node(&doc, "ghost-sys").state, NodeState::External);
        assert_eq!(doc.nodes.len(), 4);
    }

    #[tokio::test]
    async fn edges_carry_domain_and_plan_names() {
        let (_temp, doc) = fixture().await;
        let edges: Vec<(&str, &str, EdgeDomain)> = doc
            .edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str(), e.domain))
            .collect();
        assert_eq!(
            edges,
            [
                ("app", "ghost-sys", EdgeDomain::Runtime),
                ("app", "libfoo", EdgeDomain::Link),
                ("app", "libfoo", EdgeDomain::Runtime),
                ("app", "toolchain", EdgeDomain::Build),
            ],
            "plan:output collapses to plan, self-loops and duplicates drop, order is (from, to, domain)"
        );
    }

    #[tokio::test]
    async fn external_node_is_aggregated_and_metadata_free() {
        let (_temp, doc) = fixture().await;
        let ghost = node(&doc, "ghost-sys");
        assert!(ghost.version.is_none());
        assert!(ghost.outputs.is_empty());
        // Exactly one external node despite the version constraint on the dep.
        assert_eq!(
            doc.nodes
                .iter()
                .filter(|n| n.state == NodeState::External)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn multi_output_plan_lists_declared_outputs() {
        let (_temp, doc) = fixture().await;
        assert_eq!(node(&doc, "libfoo").outputs, ["core"]);
        assert_eq!(node(&doc, "app").outputs, ["app"]);
    }

    #[tokio::test]
    async fn output_is_deterministic() {
        let (_temp, doc) = fixture().await;
        let (_temp2, doc2) = fixture().await;
        assert_eq!(
            serde_json::to_string(&doc).unwrap(),
            serde_json::to_string(&doc2).unwrap()
        );
        let names: Vec<&str> = doc.nodes.iter().map(|n| n.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[tokio::test]
    async fn serialized_shape_uses_snake_case_variants() {
        let (_temp, doc) = fixture().await;
        let json = serde_json::to_value(&doc).unwrap();
        let app = &json["nodes"][0];
        assert_eq!(app["name"], "app");
        assert_eq!(app["state"], "installed");
        assert_eq!(json["edges"][1]["domain"], "link");
    }

    #[tokio::test]
    async fn render_terminal_covers_every_node() {
        let (_temp, doc) = fixture().await;
        // Smoke-level: must not panic on any state/domain combination.
        render_terminal(&doc);
        render_terminal(&GraphDoc {
            nodes: Vec::new(),
            edges: Vec::new(),
        });
    }
}

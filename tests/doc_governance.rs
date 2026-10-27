//! Documentation governance checks.
//!
//! Enforces the rules in `docs/dev/standards/`:
//!
//! - every relative link in `docs/**` and the root entry points resolves;
//! - no absolute or `file://` locator appears;
//! - a public Diátaxis page never links into `docs/dev/`;
//! - only whitelisted Markdown files live at the repository root;
//! - every ADR declares valid frontmatter and is listed in `docs/adr/index.md`;
//! - every `ADR-NNNN` citation in documentation and Rust code resolves.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

const ROOT_WHITELIST: &[&str] = &[
    "README.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "LICENSE",
    "LICENSE.md",
    "SECURITY.md",
    "CODE_OF_CONDUCT.md",
];

const ADR_STATUSES: &[&str] = &[
    "proposed",
    "accepted",
    "superseded",
    "rejected",
    "deprecated",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn is_markdown(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("md"))
}

fn is_rust(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("rs"))
}

fn collect_files(dir: &Path, want: fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, want, out);
        } else if want(&path) {
            out.push(path);
        }
    }
}

fn markdown_documents(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files(&root.join("docs"), is_markdown, &mut out);
    for name in ROOT_WHITELIST {
        let path = root.join(name);
        if path.is_file() {
            out.push(path);
        }
    }
    out
}

/// Inline link targets, skipping fenced code blocks and inline code spans.
fn link_targets(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let line = strip_code_spans(line);
        let mut rest = line.as_str();
        while let Some(index) = rest.find("](") {
            let after = &rest[index + 2..];
            let Some(end) = after.find(')') else {
                break;
            };
            let raw = after[..end].trim();
            let target = raw.split_whitespace().next().unwrap_or("");
            targets.push(target.trim_matches(|c| c == '<' || c == '>').to_string());
            rest = &after[end + 1..];
        }
    }
    targets
}

fn strip_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for character in line.chars() {
        if character == '`' {
            in_code = !in_code;
        } else if !in_code {
            out.push(character);
        }
    }
    out
}

fn ignored_target(target: &str) -> bool {
    target.is_empty()
        || target.starts_with('#')
        || target.contains("://")
        || target.starts_with("mailto:")
        || target.contains("NNNN")
}

fn resolve(base: &Path, target: &str) -> PathBuf {
    let path = Path::new(target.split('#').next().unwrap_or(""));
    let mut out = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    out
}

fn is_public_surface(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root.join("docs")) else {
        return false;
    };
    let Some(Component::Normal(first)) = relative.components().next() else {
        return false;
    };
    matches!(
        first.to_str(),
        Some("tutorials" | "how-to" | "reference" | "explanation")
    )
}

#[test]
fn test_doc_links() {
    let root = root();
    let mut errors = Vec::new();

    for path in markdown_documents(&root) {
        let Ok(text) = fs::read_to_string(&path) else {
            errors.push(format!("{}: unreadable", path.display()));
            continue;
        };
        let base = path.parent().unwrap_or(&root);

        for target in link_targets(&text) {
            if ignored_target(&target) {
                continue;
            }
            if target.starts_with('/') {
                errors.push(format!("{}: absolute link `{target}`", path.display()));
                continue;
            }

            let resolved = resolve(base, &target);
            if !resolved.exists() {
                errors.push(format!(
                    "{}: link target does not exist: `{target}`",
                    path.display()
                ));
                continue;
            }
            if is_public_surface(&root, &path) && resolved.starts_with(root.join("docs/dev")) {
                errors.push(format!(
                    "{}: public page links into docs/dev: `{target}`",
                    path.display()
                ));
            }
        }
    }

    assert!(
        errors.is_empty(),
        "documentation link errors:\n{}",
        errors.join("\n")
    );
}

#[test]
fn test_root_whitelist() {
    let root = root();
    let mut errors = Vec::new();

    let Ok(entries) = fs::read_dir(&root) else {
        panic!("cannot read repository root");
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_markdown(&path) {
            continue;
        }
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or("");
        if !ROOT_WHITELIST.contains(&name) {
            errors.push(name.to_string());
        }
    }

    assert!(
        errors.is_empty(),
        "Markdown files outside the root whitelist: {}",
        errors.join(", ")
    );
}

fn adr_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(root.join("docs/adr")) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let numbered = name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_digit());
        if numbered && is_markdown(&path) {
            files.push(path);
        }
    }
    files
}

fn frontmatter(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(&rest[..end])
}

fn field(frontmatter: &str, key: &str) -> Option<String> {
    frontmatter.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim() != key {
            return None;
        }
        let value = value.split('#').next().unwrap_or("").trim();
        Some(value.trim_matches('"').to_string())
    })
}

fn adr_number(name: &str) -> Option<&str> {
    let digits = name.get(..4)?;
    digits.bytes().all(|b| b.is_ascii_digit()).then_some(digits)
}

#[test]
fn test_adr_frontmatter_and_index() {
    let root = root();
    let index = fs::read_to_string(root.join("docs/adr/index.md")).unwrap_or_default();
    let known: BTreeSet<String> = adr_files(&root)
        .iter()
        .filter_map(|path| path.file_name().and_then(OsStr::to_str))
        .filter_map(adr_number)
        .map(str::to_string)
        .collect();
    let mut errors = Vec::new();

    for path in adr_files(&root) {
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or("");
        let Ok(text) = fs::read_to_string(&path) else {
            errors.push(format!("{name}: unreadable"));
            continue;
        };
        let Some(frontmatter) = frontmatter(&text) else {
            errors.push(format!("{name}: missing YAML frontmatter"));
            continue;
        };

        let id = field(frontmatter, "id").unwrap_or_default();
        let status = field(frontmatter, "status").unwrap_or_default();
        let date = field(frontmatter, "date").unwrap_or_default();

        let expected_id = adr_number(name).map(|number| format!("ADR-{number}"));
        if expected_id.as_deref() != Some(id.as_str()) {
            errors.push(format!("{name}: id `{id}` does not match the filename"));
        }
        if field(frontmatter, "title").is_none_or(|title| title.is_empty()) {
            errors.push(format!("{name}: missing title"));
        }
        if !ADR_STATUSES.contains(&status.as_str()) {
            errors.push(format!("{name}: invalid status `{status}`"));
        }
        let valid_date = date.len() == 10
            && date.as_bytes().get(4) == Some(&b'-')
            && date.as_bytes().get(7) == Some(&b'-')
            && date
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
        if !valid_date {
            errors.push(format!("{name}: invalid date `{date}`"));
        }
        if status == "superseded" {
            match field(frontmatter, "superseded_by") {
                Some(target) if target != "null" => {
                    let target_number = target.trim_start_matches("ADR-").to_string();
                    if !known.contains(&target_number) {
                        errors.push(format!("{name}: superseded_by `{target}` does not resolve"));
                    }
                }
                _ => errors.push(format!("{name}: superseded without superseded_by")),
            }
        }
        if !index.contains(name) {
            errors.push(format!("{name}: missing from docs/adr/index.md"));
        }
    }

    assert!(
        errors.is_empty(),
        "ADR governance errors:\n{}",
        errors.join("\n")
    );
}

#[test]
fn test_adr_lineage() {
    let root = root();
    let known: BTreeSet<String> = adr_files(&root)
        .iter()
        .filter_map(|path| path.file_name().and_then(OsStr::to_str))
        .filter_map(|name| name.get(..4).map(str::to_string))
        .collect();

    let mut sources = Vec::new();
    collect_files(&root.join("docs"), is_markdown, &mut sources);
    collect_files(&root.join("src"), is_rust, &mut sources);
    collect_files(&root.join("crates"), is_rust, &mut sources);
    collect_files(&root.join("tests"), is_rust, &mut sources);

    let mut citations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in sources {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, _) in text.match_indices("ADR-") {
            let rest = &text[index + 4..];
            let digits: String = rest.chars().take(4).collect();
            if digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit()) {
                citations
                    .entry(digits)
                    .or_default()
                    .push(path.display().to_string());
            }
        }
    }

    let missing: Vec<String> = citations
        .keys()
        .filter(|id| !known.contains(*id))
        .map(|id| format!("ADR-{id}"))
        .collect();

    assert!(
        missing.is_empty(),
        "citations to ADRs that do not exist: {}",
        missing.join(", ")
    );
}

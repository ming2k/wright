# Documentation Style & Hyperlink Standards

This document defines how Wright documentation is written, linked, and formatted.

---

## 1. Hyperlink Standards

1. **Relative paths.** Cross-document links use a path relative to the current file and include
   the `.md` extension: `[Checkpoint Recovery](../explanation/checkpoint-recovery.md)`.
2. **No absolute locators.** Do not write `/home/...`, `file://...`, or a repository URL that
   breaks offline or on a mirror.
3. **Every target exists.** A relative link must resolve to a file (or a directory containing
   an `index.md`). Dead links fail `cargo test --test doc_governance`.
4. **Fragments match headings.** A `#anchor` must match the destination heading, lower-cased
   with spaces replaced by hyphens.
5. **Descriptive link text.** `[Checkpoint Recovery](...)`, not `[here](...)`.
6. **Firewall.** Public documentation never links into `docs/dev/` (see
   [`taxonomy.md`](taxonomy.md) §2).
7. **Placeholders stay in code.** A template path such as `NNNN-slug.md` belongs inside a
   fenced block, never in a live link.

---

## 2. Markdown Conventions

1. **One H1.** Every file starts with exactly one `# Title` at line 1. Frontmatter, when
   present, precedes it.
2. **No skipped levels.** `##` sections contain `###` subsections; do not jump from `#` to
   `###`.
3. **Typed fences.** Every fenced block declares a language: `bash`, `text`, `toml`, `rust`,
   `json`, or `markdown`. Untyped fences are not accepted.
4. **Commands versus output.** A block meant to be copied contains only the command. Expected
   output goes in a separate `text` block, without shell prompts.
5. **Tables for facts.** Use a table for flags, fields, defaults, and file paths. Use prose for
   reasoning.
6. **Paths and identifiers in code.** Write `plan.toml`, `parts_dir`, and `SourceDigest` in
   backticks.

---

## 3. Voice

1. **Active and direct.** "The seal step writes the archive", not "the archive is written by
   the seal step".
2. **Imperative procedures.** Start how-to steps with the verb: "Run `wright build hello`".
3. **Second person in tutorials.** Address the reader as "you" and state the expected result.
4. **Austere reference.** No introductions, no reassurance, no narrative in reference pages.
5. **Name the mechanism.** When describing a constraint, say what enforces it: a type, a test,
   a crate boundary, or a CI gate. Avoid "should", "must remember", or "be careful".
6. **No marketing.** State what the system does and what it costs. Superlatives and promises
   are not documentation.

---

## 4. Recurring Patterns

### `README.md`

Keep it to one or two screens:

1. Title and one-sentence pitch.
2. Three to five capabilities.
3. Build and quick-start commands.
4. A documentation map linking into `docs/`.

### `CHANGELOG.md`

Follow Keep a Changelog:

- `## [Unreleased]` for changes not yet released.
- `## [X.Y.Z] - YYYY-MM-DD` for released versions, newest first.
- Subsections: `### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`,
  `### Security`.
- Released history is append-only; never rewrite a past version.

### Explanation pages

Open with the problem, not the implementation. Link an ADR when the reader needs the decision
history; keep the technical rationale in the page itself.

---

## 5. Enforcement

`cargo test --test doc_governance` checks relative link resolution, absolute and `file://`
locators, the public-to-`docs/dev` firewall, the root Markdown whitelist, and ADR frontmatter.
Formatting that is not machine-checked is a review responsibility.

---
id: ADR-0051
title: "Directory Topology Preservation and First-Class Directory Lifecycle in Stage Layers"
status: accepted
date: 2026-10-01
scope: core/foundry
superseded_by: null
negative_knowledge: true
---

# 0051. Directory Topology Preservation and First-Class Directory Lifecycle in Stage Layers

- Status: Accepted
- Date: 2026-10-01
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

Wright structures package compilation as a pipeline of discrete stages (`prepare -> configure -> compile -> staging`). In ADR-0037 ("Real directory stage working trees instead of stage overlays"), Wright adopted a merged-base architecture:
1. Each stage executes inside a real directory working tree at `target/`, populated from `base/` via copy-on-write reflinks (or hard links).
2. Upon stage exit, the engine harvests the filesystem delta into an isolated layer directory (`layers/<NN>-<stage>/`) and merges the delta into `base/`.
3. The next stage populates its `target/` from the updated `base/`.

The foundational contract between the engine and the package plan author is **stage continuity**:
> *Every filesystem state change legally produced by stage $N$ must be faithfully and transparently visible to stage $N+1$.*

Standard UNIX build systems (such as GNU Autotools, CMake, Meson, and Ninja) frequently materialize empty directory scaffolding during early stages. For example, during `./configure`, GNU Autotools creates directory trees such as `lib/deps` or `.deps/` to house future compiler dependency tracking files (`.d` files) generated during the subsequent `compile` stage.

### The Engine-Level Defect (Abstract Leaks via Git Mental Model)

Historically, `crates/wright-actions/src/foundry/layers.rs` implemented filesystem harvesting via `collect_files_recursive`. That implementation treated directories not as first-class filesystem inodes, but merely as incidental pathname prefixes of leaf files:
1. **Delta Harvesting Drop (`commit_layer`)**: `collect_files_recursive` only collected regular files and symlinks into its harvest vector. When traversing a directory, it recursed into children but never registered the directory path itself. When a directory contained no leaf files (an empty directory created by `./configure`), `commit_layer` never created the corresponding directory in `layers/<stage>/`.
2. **Target Distribution Drop (`share_tree_sync`)**: When populating `target/` from `base/`, `share_tree_sync` enqueued directories into its traversal worklist but only created target directories via `parent.create_dir_all()` when encountering leaf files. An empty directory in `base/` was popped, found to have zero entries, and completely skipped in `target/`.
3. **Deletions Blindspot**: Because directory paths were omitted from the base file inventory, explicit directory deletions (`rmdir` or `rm -rf`) in a stage failed to emit deletion tombstones into `deletions.txt`.

### Operational Impact

When building complex software like GNU Emacs or Autotools-based projects, `./configure` created `lib/deps`, but the subsequent `compile` stage saw a stripped working tree where `lib/deps` had vanished. GCC failed with cryptic errors:
```text
fatal error: lib/deps/alloca.Po: No such file or directory
```
Forcing package authors to inject synthetic `.keep` files (e.g. `touch lib/deps/.keep`) is a severe architectural failure: it pollutes declarative packaging manifests with engine-internal workarounds, transfers engine defects to the user, and violates ADR-0004 ("no implicit magic behavior").

---

## Decision Drivers

- **Strict POSIX Filesystem Fidelity**: The foundry must honor standard POSIX directory semantics: directories are first-class inodes whose existence is preserved regardless of whether they contain child entries.
- **Stage Continuity Contract**: State produced by stage $N$ must transition intact to stage $N+1$ without silent topological pruning.
- **Zero Plan Pollution**: Manifest authors must never be forced to add `.keep` or manual `mkdir` workarounds to satisfy the engine.
- **Incremental & Smart-Resume Compatibility**: Preserving directory topology must integrate seamlessly with base manifest reconciliation, checkpoint hashing, and tombstone deletion tracking.

---

## Decision Outcome

Wright refactors `crates/wright-actions/src/foundry/layers.rs` to promote directories to **first-class filesystem entities** across the entire stage lifecycle:

### 1. First-Class Tree Entry Collection (`collect_tree_entries`)

`collect_files_recursive` is replaced by `collect_tree_entries`, which partitions filesystem inventory into:
- `dirs`: all non-symlink directory paths, explicitly collected and ordered.
- `files`: all leaf nodes (regular files and symlinks).

### 2. Explicit Directory Delta Harvesting (`commit_layer`)

During `commit_layer`:
- Every directory present in `target/` that does not exist in `base/` is explicitly materialized in `layer_dir` with `std::fs::create_dir_all` and its file permissions preserved. Empty scaffolding directories (such as `lib/deps`) are thus fully captured in the layer.
- Files and symlinks continue to be captured through content comparison and copy-on-write sharing.
- Any directory or file present in `base/` that is absent in `target/` is recorded as a deletion tombstone in `deletions.txt`.

### 3. Topological Working Tree Synchronization (`share_tree_sync`)

When synchronizing trees (from `source_dir` to `base/`, or from `base/` to `target/`):
- When `ft.is_dir()` is encountered, `ensure_dest_dir(&dest_path)` is executed immediately, ensuring that empty directories are materialized in the destination with metadata intact prior to traversing any children.

### 4. Base Merging and Tombstones (`merge_layer_tree`)

- Layer directories are merged into `base/` using `ensure_dest_dir`, guaranteeing that empty layer directories are created in `base/`.
- Directory tombstones in `deletions.txt` trigger `remove_dest_any`, which recursively unlinks deleted directories from `base/`.

### System Invariants

- **`[INV-LAYER-01] Directory Inode First-Class Preservation`**: Stage delta harvesting (`commit_layer`), base tree synchronization (`share_tree_sync`), and base merging (`merge_layer_tree`) MUST treat directories as first-class filesystem entries, recording and restoring directory topology independently of whether directories contain leaf files.
- **`[INV-LAYER-02] Stage State Continuity`**: Any filesystem directory created by stage $N$ (including empty scaffolding directories created by Autotools/CMake/Ninja, e.g. `lib/deps`) MUST be faithfully preserved across the stage boundary and present in stage $N+1$'s working tree.
- **`[INV-LAYER-03] Directory Tombstone Tracking`**: Deletion of a directory (empty or populated) in stage $N$ MUST be recorded as a tombstone in `deletions.txt` and reconciled in the merged base.

---

## Rejected Alternatives & Negative Knowledge

### 1. Downstream Plan Workarounds (`.keep` files or explicit `mkdir -p`)
- **Approach**: Require plan authors to add `.keep` files in `plan.toml` or insert `mkdir -p` commands before compilation.
- **Reason for Rejection**: This represents a classic leaky abstraction (transferring engine bugs to user manifests). Autotools and standard POSIX build systems are correct; Wright's layer engine was broken. Packaging manifests must remain clean, declarative, and agnostic to engine internals.

### 2. Git-Style Path Prefix Assumption
- **Approach**: Continue assuming directories are merely parent paths of files and only synchronize leaf nodes.
- **Reason for Rejection**: Git is a content-addressable source tracking system where empty directories have no cryptographic content tree. A build workspace is an active POSIX execution environment. Conflating Git's repository format with POSIX working tree execution breaks real-world software compilation.

### 3. Reverting to Monolithic Working Trees
- **Approach**: Discard per-stage layers and run all stages sequentially in a single mutable folder.
- **Reason for Rejection**: Discarding stage layers destroys smart-resume, checkpoint rewinding, delta caching, and parallel stage provenance. First-class directory tracking achieves full POSIX fidelity while retaining all architectural benefits of real-directory stage layering (ADR-0037).

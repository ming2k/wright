---
id: ADR-0053
title: "Payload Permission Canonicalization, Deployment Healing, and Triple-Defense Umask Immunity"
status: accepted
date: 2026-10-01
scope: core/foundry, packaging, transaction
superseded_by: null
negative_knowledge: true
---

# 0053. Payload Permission Canonicalization, Deployment Healing, and Triple-Defense Umask Immunity

- Status: Accepted
- Date: 2026-10-01
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

ADR-0050 successfully established umask immunity and explicit permission contracts for forge diagnostic logs (`logs/*.log`, `0644`), system logs, and the public workshop hierarchy (`workshop/`, `0755`). However, ADR-0050 scoped its sanitization to diagnostic artifacts and workspace root directories, leaving a critical architectural blind spot in the actual package payload pipeline: **Build Execution (Sandbox) -> Package Sealing (Part Archive) -> Rootfs Deployment (Transaction)**.

In security-hardened Linux environments and standard multi-user systems, privileged administrative commands (`sudo wright install ...` or `sudo wright build ...`) inherit a strict umask from `sudo` (`Defaults umask = 0077`). This caused a multi-stage permission cascading failure across the distribution:

1. **Subprocess Execution Poisoning**: In `wright-sandbox`, child processes (native namespace PID 1 and direct fallback) inherited the caller's `umask 0077`. When standard build and installation steps (such as `make DESTDIR=$pkgdir install` or `mkdir -p`) created directories and files without explicit chmod modes, directories were created as `0700` (`drwx------ root:root`) and files as `0600` (`-rw-------`).
2. **Archive Header Contamination**: In `wright-part` (`compression.rs`), `create_tar_zst` captured raw disk filesystem metadata without canonicalization. Directory entries with `0700` were encoded directly into the tar header, contaminating the distributed `.wright.tar.zst` packages in the store.
3. **Deployment Rootfs Poisoning**: In `wright-actions` (`transaction/fs.rs`), `copy_entries_to_root` executed directory creation (`tokio::fs::create_dir_all`) under the ambient `umask 0077` of the running `sudo wright` process and never set directory permissions afterwards. Thus, any new directory branch created on the host rootfs (e.g. `/usr/libexec/mold`, `/usr/share/arca`) was written to disk as `0700`!
4. **The Latent "Survivor Bias" Trap**: Simple CLI utilities whose binaries were placed in existing shared directories (`/usr/bin`) continued to execute for regular users, hiding the fact that their documentation (`/usr/share/doc/*`), auxiliary helpers, and licenses were completely inaccessible (`0700`). The issue surfaced catastrophically when deploying complex applications like GNU Emacs (30/31), where `/usr/bin/emacs` requires immediate unprivileged access to `/usr/libexec/emacs/<version>/<arch>/emacs-*.pdmp` and `/usr/share/emacs/<version>/lisp/`, resulting in instantaneous `EACCES` crashes.

---

## Decision Drivers

- **Zero Plan Author Burden**: In every major Linux distribution (Arch, Debian, Nix, Gentoo), packagers write standard `make DESTDIR=$pkgdir install`. Packagers must never be forced to append manual `chmod -R 755` workarounds in plan recipes. Filesystem normalization is an engine invariant.
- **Triple-Defense Architecture**: Protection must exist at the source (execution environment), at the boundary (archive sealing), and at the destination (deployment transaction). No single failure mode should permit restricted permissions to escape onto the host.
- **Deterministic Canonicalization**: Packaging permissions must be canonical: standard directories `0755` (preserving setgid/sticky), executable files `0755`, and non-executable data files `0644`. World and group write bits must be stripped on non-sticky entries.
- **Self-Healing Deployment**: Deploying an updated package must automatically heal pre-existing `0700` directories created by previous buggy installations.

---

## Decision Outcome

Wright implements a comprehensive **Triple-Defense Permission Architecture** spanning the entire lifecycle from sandbox execution to host materialization:

### 1. Tier 1: Execution Environment Umask Sanitization

Before executing any build stage command or package script, Wright explicitly resets the process umask to `0022`:
- **Native Sandbox Grandchild (`native/run.rs`)**: In PID 1 of the new PID namespace, `unsafe { libc::umask(0o022); }` is called immediately post-fork and prior to `execve`.
- **Direct Execution (`direct.rs`)**: In `child.pre_exec`, `libc::umask(0o022)` is executed in the post-fork single-threaded child.
- **Deploy Hooks (`hooks.rs`)**: In `run_deploy_script`, `command.pre_exec` sets `libc::umask(0o022)`.

*(Note: Unlike ADR-0050 Rejected Alternative 2, which rejected calling `umask(2)` in the multithreaded parent runtime, calling `umask(0o022)` in the post-fork single-threaded child is POSIX-compliant, race-free, and safe.)*

### 2. Tier 2: Package Sealing Canonicalization (`wright-part`)

In `crates/wright-part/src/compression.rs`, Wright introduces canonical mode normalization for all archive entries:
- `canonicalize_dir_mode(raw_mode)`: Normalizes directory permissions to `0755` (`drwxr-xr-x`). Preserves sticky (`01000`) and setgid (`02000`) bits. Unconditionally strips group/world write on non-sticky directories.
- `canonicalize_file_mode(raw_mode)`: Normalizes executable files (`raw_mode & 0o111 != 0`) to `0755` (`-rwxr-xr-x`) and non-executable files to `0644` (`-rw-r--r--`). Preserves setuid (`04000`) and setgid (`02000`) bits while stripping dangerous group/world write permissions.
- In `create_tar_zst`: Directory and file tar headers are created with canonicalized modes.
- In `unpack_tar_safely`: Unpacking restores and enforces canonicalized modes for both files and directories, immune to ambient umask.
- In `extract_zip`: Zip extractions similarly enforce `canonicalize_dir_mode` and `canonicalize_file_mode`.

### 3. Tier 3: Deployment Materialization & Self-Healing (`wright-actions`)

In `crates/wright-actions/src/transaction/fs.rs`:
- **Phase 1 Directory Creation**: After `create_dir_all(&dest_path)`, Wright explicitly invokes `tokio::fs::set_permissions(&dest_path, Permissions::from_mode(canonicalize_dir_mode(entry.file_mode)))`. This guarantees that even if `sudo wright install` runs under `umask 0077`, newly created directories on the host rootfs are strictly `0755`.
- **Self-Healing of Existing Paths**: Because `set_permissions` is applied to all package directory entries, deploying or upgrading a package immediately heals any previously corrupted `0700` directories on the host.
- **Phase 2 File Installation**: Every regular file installed (or diverted, or backed up as `.wnew`) has its mode sanitized via `canonicalize_file_mode` before final permission application.

---

## Invariants

- **`[INV-PERM-04] Execution Environment Umask Sanitization`**: All child process executions (Sandbox PID 1, Direct execution, and Deploy hook shells) MUST explicitly reset `umask(0o022)` in the post-fork / pre-exec child process, eliminating inheritance of caller umasks (such as `sudo`'s `0077`).
- **`[INV-PERM-05] Package Sealing Permission Canonicalization`**: All package archives (`.tar.zst`) MUST canonicalize filesystem permissions during sealing: directories MUST be normalized to `0755` (preserving special bits like setgid `02000` or sticky `01000`), executable files to `0755`, and non-executable files to `0644`. Group and other write bits (`0022`) MUST be unconditionally stripped on non-sticky entries.
- **`[INV-PERM-06] Deploy Root Materialization Immunity & Healing`**: Transactional deployment to target rootfs MUST explicitly enforce `0755` on all materialized directory hierarchies and `0644`/`0755` on files, healing any pre-existing `0700` directories and preventing ambient deployment umask from poisoning host paths.
- **`[INV-PERM-07] Zero Plan Author Burden`**: Plan recipes (`plan.toml`) MUST NOT be required to include manual `chmod` workarounds for standard packaging stages. Packaging normalization is strictly an engine-level invariant.

---

## Rejected Alternatives & Negative Knowledge

### 1. Plan-Author Manual Chmod Discipline
- **Approach**: Require plan authors to write `chmod -R 755 $pkgdir` in every plan's staging stage.
- **Reason for Rejection**: Violates fundamental package management contracts. Packaging systems exist to abstract away ambient host quirks. Forcing hundreds of package recipes to maintain defensive `chmod` invocations is brittle, error-prone, and violates ADR-0004 ("no magic behavior").

### 2. Mutating Ambient Process Umask in Parent Runtime
- **Approach**: Call `libc::umask(0o022)` at startup in `src/bin/wright.rs`.
- **Reason for Rejection**: Re-affirmed from ADR-0050 Rejected Alternative 2. Modifying process-wide umask in the parent Tokio multithreaded runtime risks concurrency races with sensitive scratch directory creation (`.wright-isolation`, `0700`). Child-side reset in `pre_exec` / post-fork achieves 100% umask immunity with zero thread safety risk.

### 3. Deferring Normalization Solely to Package Sealing
- **Approach**: Only fix `create_tar_zst`, leaving Sandbox child execution and Deploy rootfs creation untouched.
- **Reason for Rejection**: Incomplete defense. If deploy runs `create_dir_all` under `sudo umask 0077`, new host directories are still born as `0700` regardless of the archive's internal mode. Furthermore, staging directories on disk would remain unreadable for local inspection.

### 4. Deferring Normalization Solely to Deployment
- **Approach**: Only fix `copy_entries_to_root`, leaving `.tar.zst` archives with broken permissions.
- **Reason for Rejection**: Contaminates distribution repositories and binary mirrors. A sealed `.tar.zst` archive must be self-contained and canonically correct when inspected or extracted with standard tools like `tar` or `bsdtar`.

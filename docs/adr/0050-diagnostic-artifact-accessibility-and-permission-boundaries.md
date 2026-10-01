---
id: ADR-0050
title: "Diagnostic artifact accessibility, explicit permission boundaries, and umask immunity"
status: accepted
date: 2026-10-01
scope: core/foundry, logging
superseded_by: null
negative_knowledge: true
---

# 0050. Diagnostic Artifact Accessibility, Explicit Permission Boundaries, and Umask Immunity

- Status: Accepted
- Date: 2026-10-01
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

Wright acts as a systems package manager and source foundry on Linux. When building packages or deploying plans, Wright constructs workspace directories under `/var/tmp/wright/workshop/<plan>-<version>` and maintains daily diagnostic logs under `/var/log/wright/wright.log.YYYY-MM-DD`.

Within each build workspace (`build_root`), Wright produces several categories of filesystem artifacts:
1. **Diagnostic & Build Logs** (`logs/compile.log`, `logs/configure.log`, `logs/slice-errors.log`): standard stdout/stderr streams, compiler diagnostics, and slice mapping logs.
2. **Public Workspace & Staging Trees** (`staging/`, `outputs/`, `source/`, `layers/`): package files, installed trees (DESTDIR), and layer snapshots intended for developer inspection and validation.
3. **Sandbox & Mount Scratch** (`.wright-isolation/`): kernel OverlayFS `upperdir`, `workdir`, and private mount namespaces.

Historically, Wright relied on standard Rust runtime primitives (`tokio::fs::create_dir_all`, `std::fs::File::create`) without explicit permission sanitization. On POSIX systems, `mkdir(2)` and `creat(2)` mask requested permissions by the active process umask (`mode & ~umask`).

In production and security-hardened Linux environments (including default systemd units, hardened shell profiles, and `sudo` configs with `Defaults umask = 0077`):
- Wright executing under privileged contexts (`sudo wright build ...` or `sudo wright install ...`) inherits `umask = 0077`.
- As a direct consequence, `/var/tmp/wright/workshop/<plan>-<version>` and its nested `logs/` directory are materialized with permissions `drwx------` (`0700`), owned by `root:root`.
- The log files (`compile.log`, `slice-errors.log`) are created with mode `-rw-------` (`0600`).
- System-wide diagnostic logs under `/var/log/wright/` are similarly restricted to `0600`.

This creates a severe operational and architectural failure mode:
- **Triage & Diagnosis Lockout**: Unprivileged users and developers cannot inspect compilation failure logs (`cat /var/tmp/wright/workshop/<pkg>/logs/compile.log` fails with `Permission denied`). This directly contradicts user documentation (e.g. `docs/how-to/write-a-plan.md`), which instructs developers to read `slice-errors.log` and stage logs directly.
- **Privilege Escalation Anti-Pattern**: Developers are forced to invoke `sudo cat` or enter interactive root shells solely to read non-sensitive compilation output, violating the principle of least privilege.
- **Architectural Ambiguity**: The access control model of the forge workspace was emergent and dependent on ambient execution state, rather than defined by explicit system invariants.

---

## Decision Drivers

- **Diagnostic Accessibility**: Package build logs and diagnostic records contain no host secrets and must be readable by any user on the system for auditing, triage, and development.
- **Fail-Safe Scratch Isolation**: OverlayFS upper layers, workdirs, and whiteout nodes inside `.wright-isolation` must remain strictly locked down (`0700`) to privileged processes.
- **Umask Immunity**: File and directory permissions must be deterministic and immune to whatever arbitrary umask (`0077`, `0027`, `0022`) is inherited from the parent shell, sudoer policy, or daemon manager.
- **Zero Ambiguity & No Magic**: In accordance with ADR-0004 ("no implicit magic behavior"), access contracts must be explicit and enforced at the point of creation.

---

## Decision Outcome

Wright explicitly bifurcates the permission boundaries within the forge workspace and diagnostic logging subsystem:

### 1. Explicit Permission Contracts

| Boundary | Target Paths | Target Mode | Rationale |
| :--- | :--- | :--- | :--- |
| **Public Workspace Tree** | `workshop/`, `<plan>-<ver>/`, `staging/`, `outputs/`, `source/`, `layers/` | `0755` (`drwxr-xr-x`) | World-traversable and readable for developer inspection and part staging verification. |
| **Diagnostic Log Hierarchy** | `workshop/<plan>-<ver>/logs/` | `0755` (`drwxr-xr-x`) | Allows unprivileged navigation into stage log directories. |
| **Diagnostic Log Files** | `.../logs/*.log`, `/var/log/wright/wright.log.*` | `0644` (`-rw-r--r--`) | World-readable compiler, slicer, and runtime diagnostics. |
| **Isolation Scratch** | `<plan>-<ver>/.wright-isolation/` and its subdirs | `0700` (`drwx------`) | Kernel OverlayFS workdir and whiteouts require strict isolation. |

### 2. Implementation: Non-Destructive Bitwise Permission Sanitization

Rather than attempting to mutate ambient process umask via `libc::umask` (which is process-wide and thread-unsafe in multi-threaded runtimes), Wright applies explicit, non-destructive bitwise permission sanitization immediately upon creation and during workspace preparation:
- Directory creation applies `mode | 0o755`. This guarantees `rwxr-xr-x` while preserving special bits (such as sticky bits `0o1777` on `/var/tmp`).
- Log file creation applies `mode | 0o644`. This guarantees `rw-r--r--` without granting group/world write permissions or spurious execution flags.
- Parent hierarchies from `build_root` down to `logs/` are validated and adjusted to ensure unprivileged traversal.

### 3. Invariants

- **`[INV-PERM-01] Diagnostic Accessibility`**: Per-stage logs (`<forge_dir>/<plan>-<version>/logs/*.log`) and system-wide diagnostic logs (`<logs_dir>/wright.log.*`) MUST be world-readable (`0644`), and their enclosing directory hierarchies MUST be world-traversable (`0755`), regardless of the execution environment's active umask.
- **`[INV-PERM-02] Scratch Boundary Isolation`**: Isolation mounts and scratch directories (`<forge_dir>/<plan>-<version>/.wright-isolation`) MUST remain strictly restricted (`0700`), preventing non-root access to kernel overlay mount points, workdirs, and whiteout devices.
- **`[INV-PERM-03] Explicit Permission Sanitization (Umask Immunity)`**: Wright MUST NOT rely on inherited process umask for filesystem structures with defined security contracts. Public and diagnostic paths must explicitly enforce `0755`/`0644`; private scratch paths must explicitly enforce `0700`.

---

## Rejected Alternatives & Negative Knowledge

### 1. Relying on Operator / Host Umask Discipline
- **Approach**: Instruct operators via documentation to run `umask 022; sudo wright ...`.
- **Reason for Rejection**: Ambient umask is routinely sanitized or reset to `0077` by `sudo` (via `secure_path` and `Defaults umask`), systemd unit defaults, or PAM modules. Relying on operator hygiene violates ADR-0004 ("no implicit magic behavior") and creates irreproducible failures across environments.

### 2. Process-Wide `libc::umask(0022)` Mutation
- **Approach**: Call `libc::umask(0o022)` during CLI startup in `src/bin/wright.rs`.
- **Reason for Rejection**: `umask(2)` in POSIX is process-global across all OS threads. Mutating process umask inside a multithreaded Tokio runtime introduces a severe race condition against parallel worker threads creating sensitive temporary files or isolation scratch structures (`.wright-isolation`), undermining security invariants.

### 3. Sudo Log Proxy / Setuid Daemon
- **Approach**: Spawn a dedicated background unprivileged helper or socket to write and serve logs.
- **Reason for Rejection**: Unnecessary architectural bloat. Standard POSIX permissions (`0644` / `0755`) completely solve unprivileged read access natively through the VFS with zero runtime overhead and zero added IPC complexity.

### 4. Restricting Logs to Root Under Security by Obscurity
- **Approach**: Maintain `0700` / `0600` on logs under the assumption that build logs could theoretically leak information.
- **Reason for Rejection**: Package manifests and build instructions in Wright are public recipes. Compiler outputs, linker lines, and packaging traces are public engineering artifacts. Hiding them behind root privilege degrades triage, impedes bug reporting, and provides no meaningful security benefit on Linux systems. Sensitive build secrets (if any) belong in secret-management injection, not in hidden compiler logs.

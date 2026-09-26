---
id: ADR-0045
title: "Deterministic ABI extraction and reverse rebuild inhibition"
status: accepted
date: 2026-03-30
scope: core/engine
superseded_by: null
negative_knowledge: true
---

# 0045. Deterministic ABI extraction and reverse rebuild inhibition

- Status: Accepted
- Date: 2026-03-30
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

When a library package (e.g. `zlib`, `openssl`, `glibc`) updates, packages that link against it (`link_deps`) may suffer runtime crashes (SIGSEGV, undefined symbol lookups) if the library's Application Binary Interface (ABI) changes incompatibly.

Conversely, upstream open-source releases do not strictly adhere to Semantic Versioning (SemVer). Patch releases frequently introduce breaking changes or bump SONAMEs, while minor or patch releases that touch only internal implementations preserve 100% of public ABI compatibility.

Historically, package managers have chosen between two failure modes:
1. **Unconditional mass rebuild (e.g. Nix/Guix)**: Every change to a dependency forces a complete rebuild of all reverse consumers. This avoids ABI breakage but causes massive build storms and wasteful compute consumption.
2. **Blind version heuristics or manual tracking (e.g. Debian, Gentoo)**: Trusting version strings or human maintainer annotations. When humans make mistakes, runtime crashes escape into production.

Wright requires a long-term, uncompromised solution that:
- Refuses to trust untrusted upstream SemVer claims.
- Guarantees 100% binary safety against dynamic linking breakage.
- Eliminates unnecessary recompilation when physical binary compatibility is proven.
- Upholds the project invariant: **Physical Facts > Declarations** (ADR-0017).

## Decision Drivers

- **Zero-trust towards upstream versions**: Never make rebuild decisions based on version numbers alone.
- **Deterministic physical verification**: Extract empirical ELF symbols, SONAMEs, and exports directly from build artifacts.
- **Fail-safe default defensive posture**: Assume ABI breakage during upgrade planning; inhibit reverse rebuilds only upon mathematical proof of compatibility.
- **Auditable metadata**: Embed physical ABI snapshots (`.ABIINFO`) into sealed parts for instantaneous compatibility checks.

## Considered Options

- **Option 1: Empirical ELF ABI Fingerprinting with Circuit-Breaker Rebuild Inhibition (Chosen)**
- **Option 2: Pure content-addressed cascading rebuilds (Nix-style mass rebuilds)**
- **Option 3: Purely manual subslots/epoch annotations in plan manifests**

## Decision Outcome

Chosen option: **Option 1: Empirical ELF ABI Fingerprinting with Circuit-Breaker Rebuild Inhibition**.

### Key Architectural Components

1. **Deterministic Physical ABI Extraction (`wright_part::abi`)**:
   - For every shared object (`.so`), goblin parses the dynamic section: `SONAME`, `DT_NEEDED`, and all defined public/weak dynamic symbols with default/protected visibility.
   - Computes a canonical SHA-256 `abi_hash` for each library and the entire part.
   - Seals `.ABIINFO` directly into `.wright.tar.zst` part archives.

2. **Ground-Truth Compatibility Evaluation (`diff_abi`)**:
   - Compares the deployed/pre-update `PartAbi` against the newly built `PartAbi`.
   - **Compatible (Identical or Superset)**: If all previous libraries exist, their SONAMEs match, and all previously exported symbols are preserved (new symbols may be added), the ABI is proven backward-compatible.
   - **Incompatible (Break)**: Any missing symbol, changed SONAME, or removed library triggers an immediate `AbiBreakReason` failure.

3. **Dynamic Rebuild Inhibition in the Engine**:
   - The execution planner begins with a **defensive posture**: reverse link dependents are scheduled in subsequent batches.
   - Prior to modifying the system, the engine records pre-update ABI snapshots of currently deployed packages.
   - Once a library wave finishes building and sealing, the engine runs the ABI diff. If proven compatible, an **inhibition signal** cancels and prunes the pending rebuilds of downstream link dependents, propagating transitively.
   - If an ABI break is detected, the inhibition signal is withheld, and downstream dependents build against the new ABI as scheduled.

4. **Plan Authority & Safety Overrides**:
   - Plans can declare `abi_epoch = N` in `plan.toml`. If `abi_epoch` increases, rebuild inhibition is bypassed regardless of symbol diffs (accounting for pure behavioral/semantic changes).
   - Plans can declare `abi_stability = "inlined"` for C++ template-heavy or header-only libraries, which disables automatic inhibition because code is baked directly into callers at compile time.
   - CLI flags (`--no-inhibit-rebuild`) allow operators to force cascading rebuilds.

### Positive Consequences

- **100% Machine-Verified Safety**: No silent runtime crashes from missing symbols or bumped SONAMEs.
- **Massive Reduction in Build Storms**: Safe minor and patch updates to widely-used libraries skip hours of redundant downstream compilations.
- **Self-Healing Transitive Invalidation**: Rebuild inhibition propagates down the dependency wave: if A is compatible, B is not rebuilt; because B did not change, C is also inhibited.
- **Zero Legacy Burden**: Fully native Rust implementation using existing goblin ELF parser without external heavy C dependencies.

### Negative Consequences

- Requires inspecting the host filesystem or `.ABIINFO` for pre-update snapshots before forging updates.
- Macro inlining changes in C headers cannot be detected by ELF symbol diffing alone; these edge cases require `abi_epoch` bumps or `abi_stability = "inlined"` annotations.

---

## Rejected Alternatives & Negative Knowledge

### Why Option 2 (Nix-Style Mass Rebuilds) Was Discarded

Unconditional content-addressed cascading rebuilds guarantee safety by treating every 1-bit change as an ABI break. In a source-based package manager operating on resource-constrained single machines, rebuilding dozens of reverse dependencies (e.g. rebuilding WebKit, LibreOffice, or GCC because of an internal zlib patch) is an unacceptable performance penalty that degrades user experience and wastes energy.

### Why Option 3 (Purely Manual Annotations) Was Discarded

Gentoo-style `subslots` and Debian symbol tracking rely on package maintainers manually discovering ABI changes and updating package files. Maintainers inevitably miss subtle ABI breaks or fail to bump subslots during minor version bumps, leading to sporadic `symbol lookup error` crashes on user systems. Delegating machine-verifiable safety to human discipline violates Wright's core principle of deterministic verification.

# Wright System Packaging & Maintenance Guide (Best Practices)

This guide is a compilation of **real-world experience** for system maintainers and packagers using `wright`. It focuses on how to maintain system consistency, handle ABI breakages, and leverage automation for large-scale updates.

---

## 1. The Core Philosophy: "Chain Consistency"

In the `wright` ecosystem, maintenance is not about updating a single package; it is about **preserving the integrity of the dependency graph**.

### 1.1 The Recursive Update Logic
When you update a low-level library (e.g., `openssl` or `libxml2`), updating the package itself is only the first step.
*   **The Concept**: Any change involving shared links (`link` dependencies) should be treated as a "root node" change.
*   **The Practice**: You should trace the dependency tree upwards and recursively update all dependent packages that link against the changed library.
*   **The Goal**: Eliminate "Symbol lookup errors" and runtime crashes caused by ABI (Application Binary Interface) mismatches.

---

## 2. Handling ABI Changes with Precision

### 2.1 Categorizing the Impact
Not every update requires a full system rebuild. Use these levels to decide your strategy:

| Change Type | Impact Level | Maintenance Strategy |
| :--- | :--- | :--- |
| **Patch Update** (v1.0.1 -> v1.0.2) | Low | Update the package; test direct dependents. |
| **Minor Update** (v1.1 -> v1.2) | Medium | Update package; `relink` or `rebuild` direct `link` dependents. |
| **Major / ABI Breakage** | High | **Trigger Recursive Update**. Treat the package as a root and update the entire reverse-dependency chain. |

### 2.2 Using Wright Commands for Recursive Updates
`wright` provides specialized flags to handle these scenarios efficiently.

#### Identify Affected Packages (Reverse Dependencies)
To see which packages need to be updated because they link against a library:
```bash
# Find all parts that link against openssl (direct only)
wright resolve openssl --rdeps=link

# Find the ENTIRE chain of packages affected by a glibc update (infinite depth)
wright resolve glibc --rdeps=link --depth=0
```

#### Apply the Recursive Update
Once you have updated the plan for a core library, use `apply` to converge the system:
```bash
# Update openssl and recursively rebuild everything that links against it
wright apply openssl --rdeps=link --depth=0
```

---

## 3. AI & Scripting: "Precision Strikes"

Manually auditing ABI changes is complex. Use AI and scripts to analyze the "Blast Radius."

### 3.1 AI-Assisted Symbol Analysis
Instead of rebuilding everything, use AI to diagnose if a rebuild is strictly necessary:
1.  **Extract Symbols**: Run `nm -D libX.so` before and after the update.
2.  **AI Diagnosis**: Feed the diff to an AI with the prompt: 
    > "Analyze the symbol changes in this library. List any removed or modified APIs. Based on the system's source code, which dependent packages are most likely to fail?"
3.  **Targeted Rebuild**: Use the AI's output to only `apply` updates to high-risk packages.

### 3.2 Post-Update Consistency Checks
After a large recursive update, run a script to verify ELF integrity:
```bash
# Example: Find any broken links in /usr/bin
find /usr/bin -type f -executable -exec ldd {} \; | grep "not found"
```

---

## 4. Maintenance Pro-Tips

*   **Don't Trust Version Numbers**: Some libraries change ABIs in minor releases. Always verify the `soname`.
*   **Leverage Link Classification**: Use `link` dependencies in your `plan.toml` instead of just `runtime`. This allows `wright` to distinguish between "I need this tool to run" and "I am compiled against this library."
*   **Atomic Transactions**: Always prefer `wright apply` over manual `build` and `install`. `apply` ensures that if a rebuild in the recursive chain fails, your system isn't left in a broken "half-updated" state.
*   **Assume Nothing**: Use `wright assume <name> <version>` for external/bootstrap packages to satisfy the dependency graph without managing them via Wright.

---

## 6. Database Maintenance & Migration

As Wright evolves, the underlying database schema may change.

### 6.1 Wright 2.x to 3.0 Migration
Version 3.0 introduces a significant database refactoring. To migrate your existing system state and archive catalogue, use the provided migration script:

```bash
# Run the migration script from the Wright project root
python3 final_migration.py
```

This script will:
1.  Back up your existing `installed.db` and `archives.db`.
2.  Create new databases consistent with the v3.0 SQL migration schema.
3.  Transfer all existing part records, file manifests, and dependency data.

### 6.2 Schema Integrity
Always ensure your system is running the expected schema version by running `wright doctor`. If schema mismatches are detected, Wright will attempt to apply pending migrations automatically, or provide instructions for manual intervention.

---

## 7. Summary: The Maintainer's Intuition

A great maintainer treats the system as a "living organism." When you touch a core library, you should expect the entire tree to vibrate. By combining **Recursive Updates** with **AI-driven Analysis**, you turn the "Dependency Hell" into a controlled, automated routine.

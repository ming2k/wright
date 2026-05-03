# How to Maintain OS Parts

This guide defines a conservative maintenance policy for operating system part sets managed with Wright. The goal is stable systems over minimal rebuild cost.

Scope:
- This document is about maintaining OS parts in your distribution plan tree.
- This document is not about maintaining the `wright` tool itself.

## 1. Update Strategy: Start From User-Facing Tools

When planning updates, target parts that are closest to users first (top of the dependency chain), such as:

- CLIs users run directly
- Services users operate directly
- End-user applications

Do not start from low-level libraries unless there is a security fix, toolchain breakage, or another hard requirement.

Why:

- User-facing updates reflect real demand.
- Required library/toolchain updates are pulled in naturally.
- You avoid random churn in deep dependencies with no user impact.

## 2. Rebuild Strategy: Pessimistic Cascading Rebuilds

If part `X` changes, rebuild every part that links to `X`, including indirect dependents.

Example:

- `a` depends on `b`
- `b` depends on `c`
- If `c` changes, rebuild `b` and `a`

Even if a part does not appear to be directly affected, treat it as affected and rebuild anyway.

This pessimistic policy is intentionally conservative to reduce ABI and integration risk.

## 3. Recommended Commands

Build the changed part and force a full dependent cascade:

```bash
wright resolve <changed-part> --rdeps=all --depth=0 | wright build --force
```

Build, then install the resulting archives:

```bash
wright resolve <changed-part> --rdeps=all --depth=0 | wright build --force --print-parts | wright install
```

When you also want a deep dependency refresh, include dependency force-rebuild:

```bash
wright resolve <changed-part> --deps=all --rdeps=all --depth=0 | wright build --force --print-parts | wright install
```

## 4. Practical Workflow

1. Choose the user-facing part you actually want to improve.
2. Update its plan/version/source as needed.
3. Run conservative rebuild with `wright resolve <part> --rdeps | wright build --force --print-parts | wright install` first; switch to `--rdeps=all --depth=0` only when the change is broader than link-ABI impact.
4. Install the printed artifacts if this is a live system update.
5. Run health checks:

```bash
wright doctor
wright resolve <changed-part> --tree --rdeps
```

## 5. Tradeoff

This policy increases build time and compute usage, but gives stronger safety for rolling upgrades and long dependency chains.

# OS Maintenance Guide (User View)

This guide defines a conservative maintenance policy for operating system part sets managed with Wright. The goal is stable systems over minimal rebuild cost.

Scope:
- This document is about maintaining OS parts in your distribution/repository.
- This document is not about maintaining the `wright` or `wbuild` tools themselves.

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

Build the changed part and force a full downstream cascade:

```bash
wbuild run <changed-part> --self --dependents=all --depth=0
```

Build + install in one pass:

```bash
wbuild run <changed-part> --self --dependents=all --depth=0 --install
```

When you also want a deep upstream refresh, include dependency force-rebuild:

```bash
wbuild run <changed-part> --self --deps=all --dependents=all --depth=0 --install
```

## 4. Practical Workflow

1. Choose the user-facing part you actually want to improve.
2. Update its plan/version/source as needed.
3. Run conservative rebuild with `--self --dependents` first; switch to `--dependents=all --depth=0` only when the change is broader than link-ABI impact.
4. Install artifacts (`--install`) if this is a live system update.
5. Run health checks:

```bash
wright doctor
wright deps <changed-part> --reverse
```

## 5. Tradeoff

This policy increases build time and compute usage, but gives stronger safety for rolling upgrades and long dependency chains.

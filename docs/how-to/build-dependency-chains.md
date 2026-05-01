# How to Build Dependency Chains

## Build a Part and All Missing Dependencies

```bash
# Resolve gtk4 plus any missing build/link deps, then build
wright resolve gtk4 --deps | wright build
```

## Build Only the Missing Deps

```bash
wright resolve gtk4 --deps | wright build
```

## Build Everything — Deps, the Part, and Dependent Link Dependents

```bash
wright resolve gtk4 --deps --rdeps | wright build
```

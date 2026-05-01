# How to Rebuild After a Library Update

A library's ABI changed. Rebuild everything that links against it:

```bash
# Update the library, then cascade to all installed link dependents
wright resolve libfoo --rdeps | wright build --force --print-parts | wright install
```

The scheduler labels affected parts as `relink` in the scheduling log.

## Full Reverse Cascade

To also catch runtime and build dependents:

```bash
wright resolve libfoo --rdeps=all --depth=0 | wright build --force --print-parts | wright install
```

## Limit Cascade Depth

```bash
wright resolve libfoo --rdeps --depth=2 | wright build --force --print-parts | wright install
```

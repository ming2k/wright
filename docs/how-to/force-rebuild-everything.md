# How to Force-Rebuild Everything from Source

Useful when shared build inputs change and you want to rebuild all parts in an assembly:

```bash
wright build @base --force
```

`--force` bypasses the archive skip check for every part in the set.

## Clean Rebuild

To also force a clean re-extraction of sources:

```bash
wright build @base --clean --force
```

This is the "start completely from scratch" option: re-extract sources and always write a new part.

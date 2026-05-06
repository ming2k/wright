# How to Force-Rebuild Everything from Source

Useful when shared build inputs change and you want to rebuild everything:

```bash
wright build --force
```

Or rebuild specific plans:

```bash
wright build zlib openssl --force
```

`--force` bypasses the archive skip check and **re-runs all lifecycle stages**
even when their sentinels exist (i.e. even when a previous build of the same
plan already completed).

## Clean Rebuild

To also force a clean re-extraction of sources:

```bash
wright build zlib openssl --clean --force
```

This is the "start completely from scratch" option: re-extract sources and always write a new part.

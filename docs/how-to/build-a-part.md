# How to Build a Part

## Minimal Plan

Minimal `plan.toml` for a C library (`zlib`):

```toml
name  = "zlib"
version = "1.3.1"
release = 1
description = "Compression library"
license = "Zlib"
arch  = "x86_64"
url   = "https://zlib.net"

[[sources]]
uri = "https://zlib.net/zlib-1.3.1.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"

[lifecycle.configure]
script = "./configure --prefix=/usr"

[lifecycle.compile]
script = "make"

[lifecycle.staging]
script = "make DESTDIR=$PART_DIR install"
```

## Fetch Checksums Automatically

```bash
wright build zlib --checksum
```

## Build

```bash
wright build zlib
```

## Build and Install Immediately

```bash
wright apply zlib
```

## Iterate on a Build Script

When tuning a stage without re-extracting sources every time, use `--stage` to run specific stages against the existing build tree:

```bash
# Full first build (extracts, configures, compiles, packages parts)
wright build mypart

# Edit lifecycle.staging in plan.toml, then re-run the output phases:
wright build mypart --stage=staging
```

To run a normal build from the start but stop after a stage so you can inspect the current `${PART_DIR}` contents, use `--until-stage`:

```bash
wright build mypart --until-stage=staging
# Inspect /var/tmp/wright/workshop/mypart-<version>/output/
```

To iterate on a subset of stages and inspect the result:

```bash
wright build mypart --stage=configure
# Inspect $WORKDIR manually
wright build mypart --stage=compile
wright build mypart --stage=staging

# Or run compile plus the output phases together:
wright build mypart --stage=compile --stage=staging
```

To skip the `check` stage:

```bash
wright build mypart --stage=prepare --stage=configure --stage=compile --stage=staging
```

Or more concisely, run the full pipeline but skip `check` by doing a full build and using `--stage` to re-run only the stages you need after a prior full configure+compile:

```bash
wright build mypart --stage=compile --stage=staging
```

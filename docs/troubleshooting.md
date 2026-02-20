# Troubleshooting

Common problems and how to diagnose them.

## Build Stage Failed

**Symptom:** `wbuild run` exits with an error like:

```
ERROR stage 'compile' failed with exit code 2
Log: /tmp/wright-build/zlib-1.3.1/log/compile.log

make: *** [Makefile:42: libz.a] Error 1
cc: error: unrecognized command-line option '-msse4'
```

The last 40 lines of output are printed inline. For the full transcript:

```bash
cat /tmp/wright-build/<name>-<version>/log/<stage>.log
```

**To re-run only the failed stage** after fixing the plan:

```bash
wbuild run <pkg> --only compile
```

**To see output live** instead of buffered to the log:

```bash
wbuild run -v <pkg>
```

Note: with multiple workers (`-w > 1`), `-v` still captures output per worker
to avoid interleaving. Run with `-w 1 -v` for fully live output.

---

## Sandbox Setup Failed

**Symptom:**

```
ERROR sandbox setup failed: unshare: Operation not permitted
```

Wright requires Linux user namespaces to run the strict sandbox. This can fail
inside containers or on kernels with namespace restrictions.

**Cause 1: unprivileged namespaces disabled**

```bash
# Check
sysctl kernel.unprivileged_userns_clone
# Fix (requires root, not persistent across reboot)
sysctl -w kernel.unprivileged_userns_clone=1
```

**Cause 2: running inside Docker/Podman without `--privileged`**

Re-run with `--privileged`, or use the `relaxed` or `none` sandbox level in
your plan while developing:

```toml
# plan.toml — per-stage sandbox override for local dev
[lifecycle.compile]
sandbox = "none"
script = "make"
```

**Cause 3: seccomp or AppArmor blocking `unshare`**

Wright automatically falls back to direct execution if namespace creation is
blocked, with a warning:

```
WARN Namespace isolation unavailable; falling back to direct execution
```

If you see this but the build still fails, the issue is elsewhere.

---

## Target Not Found

**Symptom:**

```
ERROR Target not found: mypackage
```

Wright searched all configured `plans_dir` directories and the current working
directory but could not find a `plan.toml` for `mypackage`.

**Check:**

```bash
# Verify the plan name matches the [plan] name field in plan.toml
grep '^name' path/to/mypackage/plan.toml

# Run with a path instead of a name
wbuild run ./path/to/mypackage

# Or from the plans directory root
wbuild run mypackage   # looks for plans_dir/mypackage/plan.toml
```

---

## Dependency Not Found / Missing

**Symptom:**

```
ERROR Target not found: libfoo
```

Triggered during automatic dependency expansion when a dependency declared in
`plan.toml` does not have a corresponding plan in any known plans directory.

**Options:**

1. Add the missing plan to your plans directory.
2. If the dependency is already installed on the system and you don't want to
   build it, mark it as a runtime-only dep or remove it from `build`/`link`
   dependencies if it is genuinely not needed at build time.
3. Use `--self` to skip dependency expansion entirely:
   ```bash
   wbuild run mypkg --self   # build only mypkg, assume deps are installed
   ```

---

## Deadlock Detected

**Symptom:**

```
ERROR Deadlock detected or dependency missing from plan set:
  - pkgA is waiting for: pkgB
  - pkgB is waiting for: pkgA
```

A circular dependency exists and Wright could not resolve it automatically.

**Resolution:** Add an `[mvp.dependencies]` section to one of the packages in
the cycle to declare an acyclic minimal dependency set for its first build
pass. See [writing-plans.md — Phase-Based Cycles](writing-plans.md#phase-based-cycles-mvp--full)
for the full pattern.

To inspect which cycles exist without triggering a build:

```bash
wbuild check pkgA pkgB
```

---

## Checksum Mismatch

**Symptom:**

```
ERROR SHA256 mismatch for gcc-gcc-14.2.0.tar.xz:
  expected: abc123...
  actual:   def456...
```

The downloaded file does not match the hash in `plan.toml`.

**Possible causes:**

- Upstream changed the tarball without bumping the version (uncommon but
  happens with "rolling" release tarballs).
- A partial download is in the source cache.
- The hash in `plan.toml` is wrong.

**Fix:**

```bash
# Delete the bad cached file and let Wright re-download
rm <cache_dir>/sources/<pkg>-<filename>

# Re-run to download fresh and re-verify
wbuild run <pkg>

# If you need to update the hash in plan.toml:
wbuild checksum <pkg>
```

---

## Archive Already Exists but is Wrong

**Symptom:** Build appears to succeed but the installed package is stale or
incorrect. The build log shows:

```
INFO Skipping zlib (all archives already exist, use --force to rebuild)
```

Wright found an existing archive in `components_dir` and skipped the build.
This happens after a plan edit if the version and release number were not
bumped.

**Fix:**

```bash
# Force rebuild regardless of existing archives
wbuild run <pkg> --force

# Or bump [plan] release in plan.toml to invalidate both the archive
# skip check and the build cache
```

---

## Stale Build Cache

**Symptom:** A change to a build script has no effect — the old binary is
installed. The build log shows a cache hit:

```
DEBUG Cache hit for zlib: using pre-built artifacts
```

The build cache key covers lifecycle scripts, so any script change *should*
invalidate the cache. If it doesn't, the most likely cause is that the change
was made to a field not covered by the key (e.g. a comment, or a field that
the key does not hash).

**Fix:**

```bash
# Clear the build cache and force a full recompile
wbuild run <pkg> --clean

# Also overwrite the existing output archive
wbuild run <pkg> --clean --force
```

---

## Build Directory Left in Bad State

If a build is interrupted (Ctrl-C, OOM kill, system crash), the build
directory may be partially written. On the next run Wright recreates it from
scratch, but if you hit issues:

```bash
# Manually clean the working directory for one package
wbuild run <pkg> --clean

# Or remove it directly
rm -rf /tmp/wright-build/<name>-<version>
```

---

## Wall-Clock Timeout Exceeded

**Symptom:**

```
ERROR stage 'compile' failed with exit code 137
```

Exit code 137 = killed by SIGKILL (128 + 9). Combined with a log entry:

```
ERROR Wall-clock timeout (7200s) exceeded, killing process 12345
```

The stage exceeded the `timeout` setting in `wright.toml` or `plan.toml`.

**Options:**

1. Raise `timeout` globally or for the specific package:
   ```toml
   # plan.toml
   [options]
   timeout = 14400   # 4 hours
   ```
2. Investigate why the build is slow — a hung configure script or infinite
   loop is a more likely culprit than a genuinely slow build.

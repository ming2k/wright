# Troubleshooting

Common problems and how to diagnose them.

## Build Stage Failed

**Symptom:** `wright build` exits with an error like:

```
ERROR stage 'compile' failed with exit code 2
Log: /var/tmp/wright/workshop/zlib-1.3.1/logs/compile.log

make: *** [Makefile:42: libz.a] Error 1
cc: error: unrecognized command-line option '-msse4'
```

The last 40 lines of output are printed inline. For the full transcript:

```bash
cat /var/tmp/wright/workshop/<name>-<version>/logs/<stage>.log
```

**To re-run only the failed stage** after fixing the plan:

```bash
wright build <pkg> --stage=compile
```

**To see output live** instead of buffered to the log:

```bash
wright build -v <pkg>
```

Note: when Wright is building multiple tasks in parallel, `-v` still captures
output per task to avoid interleaving. For fully live output, build a single
target or narrow the build set.

---

## Isolation Setup Failed

**Symptom:**

```
ERROR isolation setup failed: unshare: Operation not permitted
```

Wright requires Linux user namespaces to run the strict isolation. This can fail
inside containers or on kernels with namespace restrictions.

**Cause 1: unprivileged namespaces disabled**

```bash
# Check
sysctl kernel.unprivileged_userns_clone
# Fix (requires root, not persistent across reboot)
sysctl -w kernel.unprivileged_userns_clone=1
```

**Cause 2: running inside Docker/Podman without `--privileged`**

Re-run with `--privileged`, or use the `relaxed` or `none` isolation level in
your plan while developing:

```toml
# plan.toml — per-stage isolation override for local dev
[lifecycle.compile]
isolation = "none"
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
# Verify the plan name matches the top-level name field in plan.toml
grep '^name' path/to/mypackage/plan.toml

# Run with a path instead of a name
wright build ./path/to/mypackage

# Or from the plans directory root
wright build mypackage  # looks for plans_dir/mypackage/plan.toml
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
  If it is needed both at runtime and for ABI-sensitive rebuild tracking,
  declare it in both `runtime` and `link`.
3. Skip dependency expansion and build only the target:
  ```bash
  wright build mypkg
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

**Resolution:** Add an inline `[mvp.dependencies]` section or a sibling
`mvp.toml` file to one of the parts in the cycle to declare an acyclic
minimal dependency set for its first build pass. See [writing-plans.md —
Phase-Based Cycles](writing-plans.md#phase-based-cycles-mvp--full) for the
full pattern.

To inspect which cycles exist without triggering a build:

```bash
wright build pkgA pkgB --lint
```

---

## Checksum Mismatch

**Symptom:**

```
ERROR SHA256 mismatch for gcc-gcc-14.2.0.tar.xz:
 expected: abc123...
 actual:  def456...
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
rm <source_dir>/<pkg>-<filename>

# Re-run to download fresh and re-verify
wright build <pkg>

# If you need to update the hash in plan.toml:
wright build <pkg> --checksum
```

---

## Archive Already Exists but is Wrong

**Symptom:** Build appears to succeed but the installed part is stale or
incorrect. The build log shows:

```
INFO skipping batch 0: zlib (completed in previous run)
```

Wright found an existing archive in `parts_dir` and skipped the build.
This happens after a plan edit if the version and release number were not
bumped.

**Fix:**

```bash
# Force rebuild regardless of existing archives
wright build <pkg> --force

# Or bump the top-level release in plan.toml to invalidate the archive
# skip check
```

---

## Build Directory Left in Bad State

If a build is interrupted (Ctrl-C, OOM kill, system crash), the build
directory may be partially written. On the next run Wright recreates it from
scratch, but if you hit issues:

```bash
# Manually clean the working directory for one part
wright build <pkg> --clean

# Or remove it directly
rm -rf /var/tmp/wright/workshop/<name>-<version>
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

1. Raise `timeout` globally or for the specific part:
  ```toml
  # plan.toml
  [options]
  timeout = 14400  # 4 hours
  ```
2. Investigate why the build is slow — a hung configure script or infinite
  loop is a more likely culprit than a genuinely slow build.

---

## Install Fails: "failed to remove existing file … Is a directory"

**Symptom:**

```
WARN Installation failed, rolling back: install error: failed to remove existing file /usr/share/texmf: Is a directory
```

**Cause:** The part contains a path as a symlink (e.g. `ln -sfn texmf-dist texmf`), but the same path already exists on the system as a real directory from a previous install or manual action.

**Fix:** This is handled automatically as of wright 1.11.8. If you see it on an older version, remove the directory manually before installing:

```bash
rm -rf /usr/share/texmf
wright install texlive
```

---

## Installation Appears Hung After "part stored"

**Symptom:** `wright install` prints the "part stored" line and then appears to hang with one CPU core busy.

**Cause:** A `post_install` or `post_upgrade` hook is running a slow single-threaded command (common with TeX Live's `fmtutil-sys --all`, font caches, `ldconfig` on very large installs).

**Diagnosis:**

```bash
ps aux | grep -E 'fmtutil|mktexlsr|ldconfig|fc-cache'
```

**Fix:** The hook will complete on its own. For TeX Live specifically, consider removing `fmtutil-sys --all` from the hook and running only the formats you need:

```bash
sudo fmtutil-sys --byfmt pdflatex
sudo fmtutil-sys --byfmt xelatex
```

---

## Slow Install for Very Large Parts

**Symptom:** `wright install` appears to sit for a long time
on a message such as:

```text
INFO Installing texlive-texmf: 252553 files
```

Packages with hundreds of thousands of files are dominated by filesystem
metadata work and SQLite bookkeeping rather than raw CPU throughput. On SSDs,
they can still take many minutes.

**To see where time is going:**

```bash
wright -v install <pkg>
```

At `-v`, Wright prints `DEBUG` timings for:

- archive extraction
- file scan and metadata collection
- owner conflict check
- filesystem copy into the target root
- database update
- total time

If filesystem copy dominates, the bottleneck is usually the target filesystem's
metadata performance. If database update dominates, the package simply contains
an unusually large number of tracked files.

---

## Lock File Remains After Interrupt

**Symptom:** After interrupting `wright`, you still see
files such as:

```text
/var/lib/wright/lock/installed.db.lock
/var/lib/wright/lock/archives.db.lock
```

This is normal.

Wright uses fixed lock files as anchors for `flock(2)`. The presence of the
file alone does **not** mean the database is still locked. The actual lock is
held by the live process; once that process exits, the kernel releases the
lock automatically even if the `.lock` file remains on disk.

If a later command succeeds, the lock is not stale. Only investigate further if
new commands fail with a lock timeout while no Wright process is still running.

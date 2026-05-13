# Checkpoint Recovery

Wright's pipeline stage engine provides deterministic incremental rebuilds
through a content-addressed checkpoint system.  Every pipeline stage records
its execution state and input fingerprint to a JSON state machine, forming a
hash chain that detects upstream configuration changes and cascades
invalidation to all downstream stages automatically.

## Why hash chains

A naive checkpoint approach — storing only the stage name and a "completed"
flag — breaks as soon as the user edits any script or environment variable.
The next `wright build` would trust stale checkpoints, silently skipping
stages whose behaviour should have changed.

Wright solves this with **input hashing**:

```
hash(stage_N) = sha256(script_N + env_N + hash(stage_{N-1}))
```

Each stage's fingerprint incorporates:

1. The literal text of the stage's pipeline script.
2. All environment variables visible to that stage (sorted for determinism).
3. The previous stage's fingerprint, creating a blockchain-style chain.

Because every downstream hash depends on all upstream hashes, a single change
to `configure`'s flags automatically invalidates `compile`, `check`, and
`staging` — without any explicit dependency declarations.

## The `.wright-pipeline.json` state machine

Each build sandbox stores its execution history in a JSON file at the build
root:

```json
{
  "plan_name": "nginx",
  "version": "1.25.3",
  "stages": {
    "configure": {
      "status": "COMPLETED",
      "input_hash": "a1f8b32c...",
      "completed_at": "2026-05-12T10:01:20Z"
    },
    "compile": {
      "status": "FAILED",
      "input_hash": "d41d8cd9...",
      "error": "Exit code 137 (OOM)"
    }
  }
}
```

Each `stages` entry records:

| Field | Type | Meaning |
|-------|------|---------|
| `status` | `COMPLETED`, `FAILED`, or `PENDING` | Whether the stage succeeded, failed, or was never attempted |
| `input_hash` | SHA-256 hex string | The hash-chain fingerprint for this stage's inputs |
| `completed_at` | RFC-3339 timestamp | When the stage finished (only for COMPLETED) |
| `error` | string | Human-readable failure message (only for FAILED) |

The file is the **single source of truth** for the pipeline state.
Individual file-system sentinels (like the old `.wright-stage-<name>` markers)
are no longer used.

## Layered OverlayFS sandbox

Instead of a flat `work/` directory, the build sandbox uses a stack of
per-stage OverlayFS layers:

```
<build_root>/
├── .wright-pipeline.json    # Stage state machine
├── target/                  # OverlayFS merge mount point (virtual root for the container)
├── .ovl_work/               # OverlayFS working directory
├── layers/
│   ├── 01-fetch/            # Hard-links to the global source cache (no actual file copies)
│   ├── 02-verify/           # (empty — verification-only stage)
│   ├── 03-extract/          # Extracted source tree
│   ├── 04-prepare/          # Files changed by the prepare stage (e.g. patches)
│   ├── 05-configure/        # ./configure output (Makefiles, config.h)
│   ├── 06-compile/          # Compiled objects and binaries
│   ├── 07-check/            # (empty — test-only stage)
│   └── 08-staging/          # make install output (files written under /output)
├── staging/                 # Convenience alias for the final staging directory
├── outputs/                 # Sliced output directories (hard-linked from staging/)
└── logs/                    # Per-stage log files
```

### Why layered directories

The `.o` files from a failed `make` are physically isolated in
`layers/06-compile/`.  If the compile step fails:

1. The dirty layer (`06-compile`) is deleted entirely.
2. The next attempt starts with a pristine empty upperdir.
3. There is no risk of leftover `.o` files poisoning the retry — a common
   failure mode with flat `work/` directories where `make clean` is optional
   and often forgotten.

Conversely, when a stage succeeds, its layer is **frozen read-only** and
becomes part of the lowerdir stack for all subsequent stages.  Each stage only
sees the accumulated results of all previous successful stages.

### Per-stage mount / execute / commit cycle

For each stage N, the engine performs an atomic three-step cycle:

```
1. MOUNT
   lowerdir = layers/01 : layers/02 : ... : layers/N-1
   upperdir = layers/N   (must be empty)
   merged   = target/

2. EXECUTE
   Run the stage script inside target/.  All writes (creates, modifies,
   deletes) are physically redirected by the kernel into layers/N via
   OverlayFS copy-up.

3. COMMIT or ROLLBACK
   ─ On success:  Unmount target/.  Freeze layers/N — it is now a read-only
                  lowerdir for all subsequent stages.  Write COMPLETED status
                  and input_hash to .wright-pipeline.json.
   ─ On failure:  Unmount target/.  Delete layers/N/ entirely.  The next
                  attempt will create a fresh empty upperdir.
```

### Global source cache

The `fetch` stage does not copy source tarballs into the sandbox.  Instead it
creates **hard-links** from the global source cache into `layers/01-fetch/`.

The global cache lives at `~/.cache/wright/sources/` (or `/var/cache/wright/sources/`
for the system instance) and uses CAS filenames: `[sha256_hash]-[filename]`.

```
~/.cache/wright/sources/
├── a51897bf1d2e-nginx-1.25.3.tar.gz
├── b3f4a6219c8d-zlib-1.3.1.tar.xz
└── git/
    └── linux-a1b2c3d4/
```

This design decouples network downloads from sandbox pipeline:

- Deleting a build sandbox (or running `wright clean`) removes only the
  hard-links in `layers/01-fetch/`.
- The global cache files remain safe because each hard-link increments the
  inode's reference count — the file is only deleted when **all** hard-links
  (including the global cache's own directory entry) are removed.
- Multiple concurrent builds of the same source share the same inode,
  saving disk space.

## Smart resume algorithm

When `wright build` runs (with or without the `--resume` flag), the engine
performs a four-step protocol:

### 1. Compute expected hashes

For each stage in the pipeline, compute what its `input_hash`
**should be** based on the current plan manifest and environment:

```
for each stage in pipeline order:
    expected_hash[stage] = sha256(script + env + expected_hash[previous_stage])
```

### 2. Find the rewind point

Walk through `.wright-pipeline.json` from first stage to last, comparing
stored records against expected hashes:

```
rewind_point = first stage where:
    stored.status != COMPLETED
    OR stored.input_hash != expected_hash[stage]
```

A rewind happens when:
- The user edited a pipeline script (the stage's own hash changes).
- The user changed an environment variable (the stage's own hash changes).
- An upstream stage's hash changed (cascade: all downstream hashes change).
- A stage previously failed and its status is FAILED.

### 3. Rewind and clean

All stage records from the rewind point forward are reset to PENDING.
Corresponding layer directories are deleted:

```
for each stage from rewind_point to end:
    .wright-pipeline.json: set status = PENDING, clear input_hash
    rm -rf layers/<stage>/
```

Stages **before** the rewind point are preserved — their layer directories
and JSON records remain intact, avoiding redundant work.

### 4. Execute from N

The engine begins the mount/execute/commit cycle starting at the rewind
point and continues through the last stage.  The accumulated lowerdir stack
already contains all preserved layers.

## Resilience guarantees

| Scenario | Cost |
|----------|------|
| Delete build sandbox | Only hard-links in `01-fetch` are removed; global cache untouched |
| Failed `make` | Dirty `.o` files in `06-compile` are erased; clean upperdir on retry |
| Changed `./configure` flags | Everything from `configure` forward is rewound and rebuilt; `fetch` and `extract` are preserved |
| Changed source URL | The `fetch` input hash changes, triggering full rewind |
| Source tarball updated (SHA256 changed) | The global cache stores both old and new versions under different CAS paths |

## Comparison with file-sentinel checkpoints

The previous checkpoint system used per-stage sentinel files
(`.wright-stage-<name>`) storing a single fingerprint string.  That approach:

- Could not distinguish between a failed stage and a never-attempted stage
  (both were "file does not exist").
- Had no mechanism to cascade invalidation — changing `configure` did not
  automatically invalidate `compile`.
- Lacked per-stage layer isolation — failed `make` output could linger in
  a flat `work/` directory.

The hash-chain JSON model addresses all three limitations with a single
unified design.

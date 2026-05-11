# How to Rebuild Reverse Dependencies

When you update a low-level library (e.g., `openssl`, `glibc`, `libxml2`), the
package itself is only the first step. Any shared library that links against the
changed library must be rebuilt against the new ABI, or the system will produce
symbol lookup errors and runtime crashes.

## Assess the impact

Not every update requires a full cascade.

| Change Type | Impact | Strategy |
| :--- | :--- | :--- |
| **Patch** (v1.0.1 → v1.0.2) | Low | Update the plan; test direct dependents. |
| **Minor** (v1.1 → v1.2) | Medium | Update the plan; rebuild direct `link_deps` dependents. |
| **Major / ABI break** | High | Rebuild the entire reverse-dependency chain. |

To verify whether an ABI actually changed, diff the exported symbols before and
after the update:

```bash
nm -D /path/to/libfoo.so.old | sort > /tmp/syms.old
nm -D /path/to/libfoo.so.new | sort > /tmp/syms.new
diff /tmp/syms.old /tmp/syms.new
```

Removed or changed symbols mean all dependents must be rebuilt. If only symbols
were added, a full cascade is unnecessary.

## Identify affected packages

```bash
# Packages that link directly against a library
wright resolve libfoo --rdeps=link

# The entire chain, regardless of depth
wright resolve libfoo --rdeps=link --depth=0
```

## Apply the update safely

Use `wright install` with the `--rdeps` flag. It builds and installs each
dependency wave in order and leaves clear resume state if any rebuild fails, so
you never leave the system in a half-updated state.

```bash
# Rebuild only the direct link dependents
wright install libfoo --rdeps=link

# Rebuild the entire reverse-dependency chain (full cascade)
wright install libfoo --rdeps=all --depth=0

# Limit the cascade to a fixed depth
wright install libfoo --rdeps=link --depth=3
```

Prefer `wright install --rdeps` over a manual `resolve → build → package →
install` pipeline. When a rebuild fails mid-cascade, the manual pipeline leaves
uninstalled parts behind; `install` tracks which waves completed and resumes
from the failure point.

## Verify the result

After the rebuild, check that no broken links remain:

```bash
find /usr/bin /usr/lib -type f -executable -exec ldd {} \; 2>&1 | grep "not found"
```

No output means the system is consistent.

## Tips

- **Verify soname, not version.** Some libraries change ABIs in minor releases.
  Check `readelf -d libfoo.so | grep SONAME`.
- **Use `link_deps`, not `runtime_deps`.** `link_deps` tells Wright which parts
  must be rebuilt when the dependency changes. `runtime_deps` are only needed
  after installation.
- **Assume bootstrap parts.** Use `wright assume <name> <version>` (or
  `--file` for bulk) for external packages that satisfy the dependency graph
  but are not managed by Wright.

#!/bin/bash
# verify.sh — check a repository's docs/governance/documentation against the
# spec manifest. Exit 0 = compliant; exit 1 = drift, with a report.
# Usage: tools/verify.sh [repo-path]   (default: current directory)
set -u
REPO=${1:-.}
GOV=$REPO/docs/governance/documentation
if [ ! -f "$GOV/.manifest.json" ]; then
    echo "FATAL: .manifest.json missing in $GOV"
    exit 1
fi
python3 - "$GOV" <<'EOF'
import hashlib, json, os, sys
gov = sys.argv[1]
m = json.load(open(os.path.join(gov, ".manifest.json"), "r", encoding="utf-8"))

def parse_active_profiles(contracts_path):
    active = {"core"}
    if os.path.exists(contracts_path):
        with open(contracts_path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line.startswith("- [x]") or line.startswith("- [X]"):
                    rest = line[5:].strip()
                    p = rest.split("`")[1].strip() if "`" in rest else rest.split()[0].strip()
                    active.add(p)
    return active

active_profiles = parse_active_profiles(os.path.join(gov, "contracts.md"))
ok = True
print(f"governance protocol {m['protocol_version']} (schema {m['schema_version']})")
print(f"active profiles: {', '.join(sorted(active_profiles))}")

# Discover all present files recursively
present_files = set()
for root, dirs, files in os.walk(gov):
    for f in files:
        if f.startswith("."):
            continue
        rel_path = os.path.relpath(os.path.join(root, f), gov)
        present_files.add(rel_path)

expected_files = {
    name: meta for name, meta in m["files"].items()
    if meta.get("profile", "core") in active_profiles
}

all_spec_files = set(m["files"])
missing = set(expected_files) - present_files
extra = present_files - all_spec_files

if missing:
    ok = False
    print(f"  DRIFT: missing: {', '.join(sorted(missing))}")
if extra:
    ok = False
    print(f"  DRIFT: not in manifest: {', '.join(sorted(extra))}")

for name, meta in sorted(expected_files.items()):
    p = os.path.join(gov, name)
    if not os.path.exists(p):
        continue
    if meta.get("editable"):
        print(f"  [editable] {name} — local customization, not checked")
        continue
    h = hashlib.sha256(open(p, "rb").read()).hexdigest()
    if meta["sha256"] != h:
        ok = False
        print(f"  DRIFT: {name} (local {h[:12]}, manifest {meta['sha256'][:12]})")
    else:
        print(f"  ok: {name}")

sys.exit(0 if ok else 1)
EOF
rc=$?
[ $rc -ne 0 ] && { echo; echo "drift detected — run spec's tools/sync.sh, or reconcile"; echo "local edits into spec if they should propagate."; }
exit $rc

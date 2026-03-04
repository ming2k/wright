#!/usr/bin/env python3
"""Migrate plan.toml files from old schema to v1.3.1 schema.

Handles:
1. [sources] with uris/sha256 arrays -> [[sources]] array-of-tables
2. replaces/conflicts/provides in [dependencies] -> [relations]
3. [lifecycle.package] -> [package]
4. [lifecycle.package.<name>] -> [package.<name>]
5. [install_scripts] + [backup] -> [package] hooks/backup
6. [split.<name>] -> [package.<name>]
"""

import sys
import os
import glob
import tomllib


def migrate_plan(data: dict) -> dict:
    """Transform parsed plan.toml data to new v1.3.1 format."""
    out = {}

    # 1. [plan] — pass through
    if "plan" in data:
        out["plan"] = data["plan"]

    # 2. [dependencies] — extract relations fields
    relations = {}
    if "dependencies" in data:
        deps = dict(data["dependencies"])
        for field in ("replaces", "conflicts", "provides"):
            if field in deps:
                val = deps.pop(field)
                if val:
                    relations[field] = val
        out["dependencies"] = deps

    # 3. [relations] — emit if we extracted any
    if relations:
        out["relations"] = relations

    # 4. [sources] -> [[sources]]
    if "sources" in data:
        src = data["sources"]
        if isinstance(src, list):
            # Already new format
            out["sources"] = src
        elif isinstance(src, dict) and "uris" in src:
            # Old format: {uris: [...], sha256: [...]}
            uris = src.get("uris", [])
            sha256s = src.get("sha256", [])
            entries = []
            for i, uri in enumerate(uris):
                sha = sha256s[i] if i < len(sha256s) else "SKIP"
                entries.append({"uri": uri, "sha256": sha})
            out["sources"] = entries
        else:
            out["sources"] = src

    # 5. [options] — pass through
    if "options" in data:
        out["options"] = data["options"]

    # 6. [lifecycle] — pass through lifecycle stages (not package)
    if "lifecycle" in data:
        lc = dict(data["lifecycle"])
        pkg_section = lc.pop("package", None)
        if lc:
            out["lifecycle"] = lc

        # Preserve [lifecycle.package] as package output
        if pkg_section is not None:
            _merge_lifecycle_package(out, pkg_section, data)

    # 6b. Handle top-level [package] (from previous migration) -> keep as package output
    if "package" in data and "package" not in out:
        pkg = data["package"]
        if isinstance(pkg, dict):
            out["package"] = pkg

    # 7. [install_scripts] -> merge into [package] hooks
    if "install_scripts" in data:
        pkg = out.setdefault("package", {})
        hooks = pkg.setdefault("hooks", {})
        for hook_name, hook_val in data["install_scripts"].items():
            if hook_name in ("pre_install", "post_install", "post_upgrade",
                             "pre_remove", "post_remove"):
                hooks[hook_name] = hook_val

    # 8. [backup] -> merge into [package] backup
    if "backup" in data:
        backup = data["backup"]
        files = backup.get("files", []) if isinstance(backup, dict) else backup
        if files:
            pkg = out.setdefault("package", {})
            pkg["backup"] = files

    # 9. [split.<name>] -> [package.<name>]
    if "split" in data:
        pkg = out.setdefault("package", {})
        # When splits exist, the main package fields (script, hooks, backup)
        # must go under package.<main_name> (multi-package mode)
        main_name = data.get("plan", {}).get("name")
        if main_name and ("hooks" in pkg or "backup" in pkg or "script" in pkg):
            main_pkg = pkg.pop(main_name, {})
            for field in ("script", "hooks", "backup"):
                if field in pkg:
                    main_pkg[field] = pkg.pop(field)
            pkg[main_name] = main_pkg

        for split_name, split_data in data["split"].items():
            sub = {}
            if "description" in split_data:
                sub["description"] = split_data["description"]
            # [split.<name>.dependencies] -> inline
            if "dependencies" in split_data:
                sub["dependencies"] = split_data["dependencies"]
            # [split.<name>.lifecycle.package] -> script
            if "lifecycle" in split_data and "package" in split_data["lifecycle"]:
                lp = split_data["lifecycle"]["package"]
                if isinstance(lp, dict) and "script" in lp:
                    sub["script"] = lp["script"]
            # [split.<name>.install_scripts] -> hooks
            if "install_scripts" in split_data:
                sub["hooks"] = split_data["install_scripts"]
            pkg[split_name] = sub

    return out


def _merge_lifecycle_package(out: dict, pkg_section, data: dict):
    """Merge [lifecycle.package] into [package]."""
    pkg = out.setdefault("package", {})

    if isinstance(pkg_section, dict):
        # Check if it's a single package (has "script" key) or multi-package
        if "script" in pkg_section or "hooks" in pkg_section:
            # Single package: [lifecycle.package] with script/hooks
            for k, v in pkg_section.items():
                pkg[k] = v
        else:
            # Could be multi-package: [lifecycle.package.<name>]
            # Check if values are dicts (sub-packages)
            all_dicts = all(isinstance(v, dict) for v in pkg_section.values())
            if all_dicts and pkg_section:
                for name, sub in pkg_section.items():
                    pkg[name] = sub
            else:
                # Single package with other fields
                for k, v in pkg_section.items():
                    pkg[k] = v


def emit_toml(data: dict) -> str:
    """Emit a plan.toml in the canonical v1.3.1 order."""
    lines = []

    # [plan]
    if "plan" in data:
        lines.append("[plan]")
        lines.extend(_emit_flat_fields(data["plan"]))
        lines.append("")

    # [dependencies]
    if "dependencies" in data:
        lines.append("[dependencies]")
        deps = data["dependencies"]
        # Emit in preferred order
        for key in ("runtime", "build", "link"):
            if key in deps:
                lines.append(f"{key} = {_fmt_val(deps[key])}")
        # Any other fields
        for key, val in deps.items():
            if key not in ("runtime", "build", "link"):
                lines.append(f"{key} = {_fmt_val(val)}")
        lines.append("")

    # [relations]
    if "relations" in data:
        lines.append("[relations]")
        rels = data["relations"]
        for key in ("replaces", "conflicts", "provides"):
            if key in rels:
                lines.append(f"{key} = {_fmt_val(rels[key])}")
        lines.append("")

    # [[sources]]
    if "sources" in data:
        sources = data["sources"]
        if isinstance(sources, list):
            for entry in sources:
                lines.append("[[sources]]")
                if isinstance(entry, dict):
                    lines.append(f'uri = {_fmt_val(entry.get("uri", ""))}')
                    lines.append(f'sha256 = {_fmt_val(entry.get("sha256", "SKIP"))}')
                lines.append("")

    # [options]
    if "options" in data:
        lines.append("[options]")
        lines.extend(_emit_flat_fields(data["options"]))
        lines.append("")

    # [lifecycle.*] stages
    if "lifecycle" in data:
        stage_order = ["prepare", "configure", "compile", "check", "staging"]
        lc = data["lifecycle"]
        # Emit in order
        for stage in stage_order:
            if stage in lc:
                lines.append(f"[lifecycle.{stage}]")
                lines.extend(_emit_lifecycle_fields(lc[stage]))
                lines.append("")
        # Any other stages not in the standard order
        for stage, val in lc.items():
            if stage not in stage_order:
                lines.append(f"[lifecycle.{stage}]")
                lines.extend(_emit_lifecycle_fields(val))
                lines.append("")

    # [lifecycle.package] or [lifecycle.package.<name>]
    if "package" in data:
        pkg = data["package"]
        if _is_single_package(pkg):
            lines.append("[lifecycle.package]")
            lines.extend(_emit_package_fields(pkg))
            lines.append("")
        else:
            for name, sub in pkg.items():
                if isinstance(sub, dict):
                    lines.append(f"[lifecycle.package.{_quote_key(name)}]")
                    lines.extend(_emit_package_fields(sub))
                    lines.append("")

    # Remove trailing blank lines, ensure single trailing newline
    while lines and lines[-1] == "":
        lines.pop()
    return "\n".join(lines) + "\n"


def _is_single_package(pkg: dict) -> bool:
    """Check if package dict is single-package mode (has direct fields like script/hooks)."""
    # If any value is a dict, it's likely multi-package (sub-package entries)
    if any(isinstance(v, dict) and k not in ("hooks",) for k, v in pkg.items()):
        return False
    direct_keys = {"script", "hooks", "backup", "description"}
    return bool(direct_keys & set(pkg.keys()))


def _quote_key(name: str) -> str:
    """Quote a TOML key if needed."""
    if all(c.isalnum() or c in "-_" for c in name):
        return name
    return f'"{name}"'


def _fmt_val(val) -> str:
    """Format a TOML value."""
    if isinstance(val, str):
        if "\n" in val:
            # Use literal strings (''') for multi-line to avoid backslash escaping
            return f"'''\n{val}'''"
        if "\\" in val:
            # Use literal string for backslash-heavy single-line strings
            return f"'{val}'"
        return f'"{_escape_str(val)}"'
    if isinstance(val, bool):
        return "true" if val else "false"
    if isinstance(val, int):
        return str(val)
    if isinstance(val, float):
        return str(val)
    if isinstance(val, list):
        return _fmt_array(val)
    if isinstance(val, dict):
        return _fmt_inline_table(val)
    return repr(val)


def _escape_str(s: str) -> str:
    """Escape a TOML basic string."""
    return s.replace("\\", "\\\\").replace('"', '\\"')


def _fmt_array(arr: list) -> str:
    """Format a TOML array."""
    if not arr:
        return "[]"
    # Check if all elements are simple strings
    if all(isinstance(x, str) and "\n" not in x for x in arr):
        if len(arr) == 1:
            return f'["{_escape_str(arr[0])}"]'
        if sum(len(x) for x in arr) < 60:
            items = ", ".join(f'"{_escape_str(x)}"' for x in arr)
            return f"[{items}]"
        # Multi-line
        lines = ["["]
        for x in arr:
            lines.append(f'    "{_escape_str(x)}",')
        lines.append("]")
        return "\n".join(lines)
    # Mixed types or complex
    items = ", ".join(_fmt_val(x) for x in arr)
    return f"[{items}]"


def _fmt_inline_table(d: dict) -> str:
    """Format a TOML inline table."""
    items = ", ".join(f"{k} = {_fmt_val(v)}" for k, v in d.items())
    return f"{{{items}}}"


def _emit_flat_fields(d: dict) -> list[str]:
    """Emit flat key=value lines for a dict."""
    lines = []
    for k, v in d.items():
        lines.append(f"{k} = {_fmt_val(v)}")
    return lines


def _emit_lifecycle_fields(stage: dict) -> list[str]:
    """Emit lifecycle stage fields, with script last."""
    lines = []
    for k, v in stage.items():
        if k == "script":
            continue
        lines.append(f"{k} = {_fmt_val(v)}")
    if "script" in stage:
        lines.append(f"script = {_fmt_val(stage['script'])}")
    return lines


def _emit_package_fields(pkg: dict) -> list[str]:
    """Emit fields for a [package] or [package.<name>] section."""
    lines = []
    if "description" in pkg:
        lines.append(f'description = {_fmt_val(pkg["description"])}')
    if "dependencies" in pkg:
        deps = pkg["dependencies"]
        for dk, dv in deps.items():
            lines.append(f"dependencies.{dk} = {_fmt_val(dv)}")
    if "hooks" in pkg:
        hooks = pkg["hooks"]
        for hk in ("pre_install", "post_install", "post_upgrade", "pre_remove", "post_remove"):
            if hk in hooks:
                lines.append(f"hooks.{hk} = {_fmt_val(hooks[hk])}")
    if "backup" in pkg:
        lines.append(f"backup = {_fmt_val(pkg['backup'])}")
    if "script" in pkg:
        lines.append(f"script = {_fmt_val(pkg['script'])}")
    return lines


def process_file(filepath: str, dry_run: bool = False) -> tuple[bool, str]:
    """Process a single plan.toml. Returns (changed, message)."""
    with open(filepath, "r") as f:
        content = f.read()

    # Already fully migrated? (has [[sources]] AND no top-level [package] without lifecycle prefix)
    has_new_sources = "[[sources]]" in content
    import re
    has_toplevel_package = bool(re.search(r'^\[package[\].]', content, re.MULTILINE))
    has_lifecycle_package = "[lifecycle.package]" in content or "[lifecycle.package." in content
    if has_new_sources and not has_toplevel_package and (has_lifecycle_package or "[package" not in content):
        return False, "already migrated"

    try:
        data = tomllib.loads(content)
    except Exception as e:
        return False, f"parse error: {e}"

    new_data = migrate_plan(data)
    new_content = emit_toml(new_data)

    # Verify the output parses correctly
    try:
        roundtrip = tomllib.loads(new_content)
    except Exception as e:
        return False, f"roundtrip parse error: {e}"

    if new_content == content:
        return False, "no changes"

    if not dry_run:
        with open(filepath, "w") as f:
            f.write(new_content)

    return True, "migrated"


def main():
    if len(sys.argv) < 2:
        print("Usage: migrate-plans.py <plans-dir> [--dry-run] [--verbose]")
        sys.exit(1)

    plans_dir = sys.argv[1]
    dry_run = "--dry-run" in sys.argv
    verbose = "--verbose" in sys.argv

    plan_files = sorted(glob.glob(os.path.join(plans_dir, "*/plan.toml")))
    print(f"Found {len(plan_files)} plan files")

    migrated = 0
    skipped = 0
    errors = 0
    for filepath in plan_files:
        pkg_name = os.path.basename(os.path.dirname(filepath))
        try:
            changed, msg = process_file(filepath, dry_run=dry_run)
            if changed:
                prefix = "WOULD MIGRATE" if dry_run else "MIGRATED"
                print(f"  [{prefix}] {pkg_name}")
                migrated += 1
            else:
                if verbose:
                    print(f"  [SKIP] {pkg_name}: {msg}")
                skipped += 1
        except Exception as e:
            print(f"  [ERROR] {pkg_name}: {e}", file=sys.stderr)
            errors += 1

    print(f"\nTotal: {len(plan_files)}, Migrated: {migrated}, Skipped: {skipped}, Errors: {errors}")
    if dry_run:
        print("(dry run -- no files were modified)")


if __name__ == "__main__":
    main()

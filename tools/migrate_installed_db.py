#!/usr/bin/env python3
from __future__ import annotations

import argparse
import shutil
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path


def has_table(conn: sqlite3.Connection, table: str) -> bool:
    row = conn.execute(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        (table,),
    ).fetchone()
    return bool(row and row[0])


def table_columns(conn: sqlite3.Connection, table: str) -> set[str]:
    if not has_table(conn, table):
        return set()
    rows = conn.execute(f"PRAGMA table_info({table})").fetchall()
    return {row[1] for row in rows}


def ensure_schema(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE TABLE IF NOT EXISTS parts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            version TEXT NOT NULL,
            release INTEGER NOT NULL,
            description TEXT,
            arch TEXT NOT NULL,
            license TEXT,
            url TEXT,
            installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            install_size INTEGER,
            pkg_hash TEXT,
            install_scripts TEXT,
            assumed INTEGER NOT NULL DEFAULT 0,
            origin TEXT NOT NULL DEFAULT 'manual',
            epoch INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            part_id INTEGER NOT NULL,
            path TEXT NOT NULL,
            file_hash TEXT,
            file_type TEXT NOT NULL,
            file_mode INTEGER,
            file_size INTEGER,
            is_config BOOLEAN DEFAULT 0,
            FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            part_id INTEGER NOT NULL,
            depends_on TEXT NOT NULL,
            version_constraint TEXT,
            dep_type TEXT DEFAULT 'runtime',
            FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS transactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
            operation TEXT NOT NULL,
            part_name TEXT NOT NULL,
            old_version TEXT,
            new_version TEXT,
            status TEXT NOT NULL,
            backup_path TEXT
        );

        CREATE TABLE IF NOT EXISTS optional_dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            part_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS provides (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            part_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS conflicts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            part_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS shadowed_files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL,
            original_owner_id INTEGER NOT NULL,
            shadowed_by_id INTEGER NOT NULL,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (original_owner_id) REFERENCES parts(id) ON DELETE CASCADE,
            FOREIGN KEY (shadowed_by_id) REFERENCES parts(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_files_package ON files(part_id);
        CREATE INDEX IF NOT EXISTS idx_deps_package ON dependencies(part_id);
        CREATE INDEX IF NOT EXISTS idx_deps_on ON dependencies(depends_on);
        CREATE INDEX IF NOT EXISTS idx_opt_deps_package ON optional_dependencies(part_id);
        CREATE INDEX IF NOT EXISTS idx_provides_name ON provides(name);
        CREATE INDEX IF NOT EXISTS idx_provides_package ON provides(part_id);
        CREATE INDEX IF NOT EXISTS idx_conflicts_name ON conflicts(name);
        CREATE INDEX IF NOT EXISTS idx_conflicts_package ON conflicts(part_id);
        CREATE INDEX IF NOT EXISTS idx_shadowed_path ON shadowed_files(path);
        PRAGMA foreign_keys = ON;
        """
    )


def backup_destination(dest: Path) -> Path | None:
    if not dest.exists():
        return None
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    backup_dir = dest.parent / "backups"
    backup_dir.mkdir(parents=True, exist_ok=True)
    backup_path = backup_dir / f"{dest.stem}-before-migration-{stamp}{dest.suffix}"
    shutil.copy2(dest, backup_path)
    for suffix in ("-wal", "-shm", "-journal"):
        sidecar = Path(f"{dest}{suffix}")
        if sidecar.exists():
            shutil.copy2(sidecar, backup_dir / f"{sidecar.name}.{stamp}")
    return backup_path


def import_parts(src: sqlite3.Connection, dst: sqlite3.Connection, replace: bool) -> tuple[dict[int, int], dict[str, int]]:
    src_table = "packages" if has_table(src, "packages") else "parts"
    cols = table_columns(src, src_table)
    id_map: dict[int, int] = {}
    name_to_new_id: dict[str, int] = {}

    for row in src.execute(f"SELECT * FROM {src_table} ORDER BY name"):
        record = dict(row)
        name = record["name"]
        old_id = record["id"]
        existing = dst.execute("SELECT id FROM parts WHERE name = ?", (name,)).fetchone()
        if existing and not replace:
            id_map[old_id] = existing["id"]
            continue
        if existing and replace:
            dst.execute("DELETE FROM parts WHERE name = ?", (name,))

        payload = {
            "name": name,
            "version": record["version"],
            "release": record.get("release", 0),
            "epoch": record.get("epoch", 0) if "epoch" in cols else 0,
            "description": record.get("description", "") or "",
            "arch": record.get("arch", "any") or "any",
            "license": record.get("license", "") or "",
            "url": record.get("url") if "url" in cols else None,
            "install_size": record.get("install_size", 0) if "install_size" in cols else 0,
            "pkg_hash": record.get("pkg_hash") if "pkg_hash" in cols else None,
            "install_scripts": record.get("install_scripts") if "install_scripts" in cols else None,
            "assumed": record.get("assumed", 0) if "assumed" in cols else 0,
            "origin": record.get("install_reason", "manual") if "install_reason" in cols else "manual",
            "installed_at": record.get("installed_at") if "installed_at" in cols else None,
        }
        cur = dst.execute(
            """
            INSERT INTO parts (
                name, version, release, epoch, description, arch, license, url,
                installed_at, install_size, pkg_hash, install_scripts, assumed, origin
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, CURRENT_TIMESTAMP), ?, ?, ?, ?, ?)
            """,
            (
                payload["name"],
                payload["version"],
                payload["release"],
                payload["epoch"],
                payload["description"],
                payload["arch"],
                payload["license"],
                payload["url"],
                payload["installed_at"],
                payload["install_size"],
                payload["pkg_hash"],
                payload["install_scripts"],
                payload["assumed"],
                payload["origin"],
            ),
        )
        new_id = cur.lastrowid
        id_map[old_id] = new_id
        name_to_new_id[name] = new_id

    return id_map, name_to_new_id


def copy_child_table(
    src: sqlite3.Connection,
    dst: sqlite3.Connection,
    src_table: str,
    dst_sql: str,
    fk_candidates: tuple[str, ...],
    row_mapper,
) -> int:
    if not has_table(src, src_table):
        return 0
    cols = table_columns(src, src_table)
    fk_col = next((name for name in fk_candidates if name in cols), None)
    if not fk_col:
        return 0

    count = 0
    for row in src.execute(f"SELECT * FROM {src_table} ORDER BY {fk_col}, id"):
        mapped = row_mapper(dict(row), fk_col)
        if mapped is None:
            continue
        dst.execute(dst_sql, mapped)
        count += 1
    return count


def import_database(source: Path, dest: Path, replace: bool, backup: bool) -> None:
    if not source.exists():
        raise SystemExit(f"source database does not exist: {source}")

    src = sqlite3.connect(source)
    src.row_factory = sqlite3.Row

    if not (has_table(src, "packages") or has_table(src, "parts")):
        raise SystemExit(f"{source} does not look like a Wright installed-state database")

    dest.parent.mkdir(parents=True, exist_ok=True)
    backup_path = backup_destination(dest) if backup else None

    dst = sqlite3.connect(dest)
    dst.row_factory = sqlite3.Row
    try:
        ensure_schema(dst)
        dst.execute("BEGIN")

        id_map, _ = import_parts(src, dst, replace)

        file_count = copy_child_table(
            src,
            dst,
            "files",
            "INSERT INTO files (part_id, path, file_hash, file_type, file_mode, file_size, is_config) VALUES (?, ?, ?, ?, ?, ?, ?)",
            ("package_id", "part_id"),
            lambda row, fk: (
                id_map[row[fk]],
                row["path"],
                row.get("file_hash"),
                row.get("file_type", "file"),
                row.get("file_mode"),
                row.get("file_size"),
                row.get("is_config", 0),
            )
            if row[fk] in id_map
            else None,
        )

        dep_count = copy_child_table(
            src,
            dst,
            "dependencies",
            "INSERT INTO dependencies (part_id, depends_on, version_constraint, dep_type) VALUES (?, ?, ?, ?)",
            ("package_id", "part_id"),
            lambda row, fk: (
                id_map[row[fk]],
                row["depends_on"],
                row.get("version_constraint"),
                row.get("dep_type", "runtime"),
            )
            if row[fk] in id_map
            else None,
        )

        tx_cols = table_columns(src, "transactions")
        tx_name_col = "package_name" if "package_name" in tx_cols else "part_name" if "part_name" in tx_cols else None
        tx_count = 0
        if tx_name_col:
            for row in src.execute("SELECT * FROM transactions ORDER BY timestamp, id"):
                record = dict(row)
                dst.execute(
                    "INSERT INTO transactions (operation, part_name, old_version, new_version, status, backup_path) VALUES (?, ?, ?, ?, ?, ?)",
                    (
                        record["operation"],
                        record[tx_name_col],
                        record.get("old_version"),
                        record.get("new_version"),
                        record.get("status", "completed"),
                        record.get("backup_path"),
                    ),
                )
                tx_count += 1

        dst.commit()
    except Exception:
        dst.rollback()
        raise
    finally:
        src.close()
        dst.close()

    if backup_path:
        print(f"backup: {backup_path}")
    print(f"source: {source}")
    print(f"dest:   {dest}")
    print(f"imported files: {file_count}")
    print(f"imported dependencies: {dep_count}")
    print(f"imported transactions: {tx_count}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Migrate a historical Wright installed-state SQLite database into installed.db."
    )
    parser.add_argument("source", type=Path, help="path to the legacy packages.db or older installed-state DB")
    parser.add_argument(
        "--dest",
        type=Path,
        default=Path("/var/lib/wright/state/installed.db"),
        help="destination installed.db path (default: /var/lib/wright/state/installed.db)",
    )
    parser.add_argument(
        "--replace",
        action="store_true",
        help="replace same-named parts already present in the destination DB",
    )
    parser.add_argument(
        "--no-backup",
        action="store_true",
        help="do not create a backup copy of the destination DB before writing",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    import_database(args.source, args.dest, args.replace, not args.no_backup)
    return 0


if __name__ == "__main__":
    sys.exit(main())

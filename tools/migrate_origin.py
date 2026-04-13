#!/usr/bin/env python3
"""Migrate parts table: rename install_reason column to origin, convert 'explicit' → 'manual'.

SQLite does not support ALTER COLUMN RENAME, so this rebuilds the table.

Usage:
    python3 tools/migrate_origin.py /path/to/installed.db
"""

import shutil
import sqlite3
import sys


def migrate(db_path: str) -> None:
    # Check if migration is needed
    conn = sqlite3.connect(db_path)
    cols = [row[1] for row in conn.execute("PRAGMA table_info(parts)").fetchall()]

    if "origin" in cols and "install_reason" not in cols:
        print("Already migrated.")
        conn.close()
        return

    if "install_reason" not in cols:
        print("Error: no install_reason column found.", file=sys.stderr)
        conn.close()
        sys.exit(1)

    # Backup
    backup_path = db_path + ".bak"
    shutil.copy2(db_path, backup_path)
    print(f"Backup: {backup_path}")

    conn.execute("PRAGMA foreign_keys = OFF;")

    conn.execute("""
        CREATE TABLE parts_new (
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
        )
    """)

    conn.execute("""
        INSERT INTO parts_new (id, name, version, release, description, arch, license, url,
                               installed_at, install_size, pkg_hash, install_scripts, assumed,
                               origin, epoch)
        SELECT id, name, version, release, description, arch, license, url,
               installed_at, install_size, pkg_hash, install_scripts, assumed,
               CASE install_reason WHEN 'explicit' THEN 'manual' ELSE install_reason END,
               epoch
        FROM parts
    """)

    conn.execute("DROP TABLE parts")
    conn.execute("ALTER TABLE parts_new RENAME TO parts")

    conn.execute("PRAGMA foreign_keys = ON;")
    conn.commit()

    count = conn.execute("SELECT COUNT(*) FROM parts").fetchone()[0]
    conn.close()
    print(f"Migrated {count} part(s): install_reason → origin, explicit → manual")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <installed.db>", file=sys.stderr)
        sys.exit(1)
    migrate(sys.argv[1])

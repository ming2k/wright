import sqlite3
import os
import sys
import hashlib

COPY_BATCH_SIZE = 1000

# Configuration
OLD_STATE_DIR = "/var/lib/wright/state"
NEW_INSTALLED_DB = os.path.join(OLD_STATE_DIR, "installed.db")
OLD_INSTALLED_DB = os.path.join(OLD_STATE_DIR, "installed.db.bak")
NEW_ARCHIVES_DB = os.path.join(OLD_STATE_DIR, "archives.db")
OLD_ARCHIVES_DB = os.path.join(OLD_STATE_DIR, "archives.db.bak")

# Schema paths from the refactored source code
INSTALLED_SCHEMA_PATH = "src/database/migrations/001_initial_schema.sql"
ARCHIVE_SCHEMA_PATH = "src/database/migrations/archive/001_initial_archive_schema.sql"
INITIAL_INSTALLED_MIGRATION_DESCRIPTION = "initial schema"
INITIAL_ARCHIVE_MIGRATION_DESCRIPTION = "initial archive schema"

def register_sqlx_baseline(conn, version, description, sql):
    checksum = hashlib.sha384(sql.encode("utf-8")).digest()
    conn.execute("""
        CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )
    """)
    conn.execute("""
        INSERT OR REPLACE INTO _sqlx_migrations
            (version, description, installed_on, success, checksum, execution_time)
        VALUES (?, ?, CURRENT_TIMESTAMP, 1, ?, 0)
    """, (version, description, checksum))

def table_exists(cursor, table_name):
    row = cursor.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
        (table_name,),
    ).fetchone()
    return row is not None

def get_common_columns(cursor_old, cursor_new, table_name):
    if not table_exists(cursor_old, table_name) or not table_exists(cursor_new, table_name):
        return []

    cursor_old.execute(f"PRAGMA table_info({table_name})")
    old_cols = {row[1] for row in cursor_old.fetchall()}
    cursor_new.execute(f"PRAGMA table_info({table_name})")
    new_cols = {row[1] for row in cursor_new.fetchall()}
    return sorted(list(old_cols.intersection(new_cols)))

def copy_table_rows(old_conn, new_conn, table_name, common_columns):
    cols_str = ", ".join(common_columns)
    placeholders = ", ".join(["?" for _ in common_columns])
    select_sql = f"SELECT {cols_str} FROM {table_name}"
    insert_sql = f"INSERT INTO {table_name} ({cols_str}) VALUES ({placeholders})"

    cursor = old_conn.execute(select_sql)
    copied_rows = 0

    while True:
        rows = cursor.fetchmany(COPY_BATCH_SIZE)
        if not rows:
            break
        new_conn.executemany(insert_sql, rows)
        copied_rows += len(rows)

    return copied_rows

def cleanup_file(path):
    try:
        os.remove(path)
    except FileNotFoundError:
        pass

def migrate_db(old_path, new_path, schema_sql, table_list, migration_description):
    print(f"\n>>> Migrating {os.path.basename(old_path)} -> {os.path.basename(new_path)}")

    if not os.path.exists(old_path):
        print(f"Warning: Old database {old_path} not found. Skipping.")
        return

    state_dir = os.path.dirname(new_path)
    if state_dir:
        os.makedirs(state_dir, exist_ok=True)

    temp_path = f"{new_path}.tmp"
    cleanup_file(temp_path)

    try:
        with sqlite3.connect(old_path) as old_conn, sqlite3.connect(temp_path) as new_conn:
            # Initialize schema using the exact same SQL files used by Wright.
            new_conn.executescript(schema_sql)
            # Record the initial migration in SQLx's migration ledger so runtime
            # startup does not try to replay 001_* against an already-populated schema.
            register_sqlx_baseline(new_conn, 1, migration_description, schema_sql)

            old_cursor = old_conn.cursor()
            new_cursor = new_conn.cursor()

            for table in table_list:
                common = get_common_columns(old_cursor, new_cursor, table)
                if not common:
                    print(f"    Table '{table}' skipped (not found or no common fields).")
                    continue

                cols_str = ", ".join(common)
                print(f"    Table '{table}': migrating fields [{cols_str}]")

                copied_rows = copy_table_rows(old_conn, new_conn, table, common)
                print(f"    Successfully migrated {copied_rows} records into '{table}'.")

            new_conn.commit()

        os.replace(temp_path, new_path)
    except Exception:
        cleanup_file(temp_path)
        raise

def main():
    # Verify we are in the project root
    if not os.path.exists(INSTALLED_SCHEMA_PATH) or not os.path.exists(ARCHIVE_SCHEMA_PATH):
        print("Error: Could not find schema files. Please run this script from the Wright project root.")
        sys.exit(1)

    # Load schemas
    with open(INSTALLED_SCHEMA_PATH, "r") as f:
        installed_schema = f.read()
    with open(ARCHIVE_SCHEMA_PATH, "r") as f:
        archive_schema = f.read()

    # 1. Migrate Installed DB (System state)
    installed_tables = [
        "parts", "files", "dependencies", "transactions", "shadowed_files",
        "optional_dependencies", "provides", "conflicts", "replaces", "build_sessions"
    ]
    migrate_db(
        OLD_INSTALLED_DB,
        NEW_INSTALLED_DB,
        installed_schema,
        installed_tables,
        INITIAL_INSTALLED_MIGRATION_DESCRIPTION,
    )

    # 2. Migrate Archives DB (Local archive catalogue)
    archive_tables = ["parts", "dependencies", "provides", "conflicts", "replaces"]
    migrate_db(
        OLD_ARCHIVES_DB,
        NEW_ARCHIVES_DB,
        archive_schema,
        archive_tables,
        INITIAL_ARCHIVE_MIGRATION_DESCRIPTION,
    )

    print("\n[SUCCESS] Migration complete. Both databases are now clean V1 and consistent with the new architecture.")

if __name__ == "__main__":
    main()

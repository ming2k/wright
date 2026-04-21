import sqlite3
import os
import sys

# Configuration
OLD_STATE_DIR = "/var/lib/wright/state"
NEW_INSTALLED_DB = os.path.join(OLD_STATE_DIR, "installed.db")
OLD_INSTALLED_DB = os.path.join(OLD_STATE_DIR, "installed.db.bak")
NEW_ARCHIVES_DB = os.path.join(OLD_STATE_DIR, "archives.db")
OLD_ARCHIVES_DB = os.path.join(OLD_STATE_DIR, "archives.db.bak")

# Schema paths from the refactored source code
INSTALLED_SCHEMA_PATH = "src/database/migrations/001_initial_schema.sql"
ARCHIVE_SCHEMA_PATH = "src/database/migrations/archive/001_initial_archive_schema.sql"

def get_common_columns(cursor_old, cursor_new, table_name):
    try:
        cursor_old.execute(f"PRAGMA table_info({table_name})")
        old_cols = {row[1] for row in cursor_old.fetchall()}
        cursor_new.execute(f"PRAGMA table_info({table_name})")
        new_cols = {row[1] for row in cursor_new.fetchall()}
        return sorted(list(old_cols.intersection(new_cols)))
    except:
        return []

def migrate_db(old_path, new_path, schema_sql, table_list):
    print(f"\n>>> Migrating {os.path.basename(old_path)} -> {os.path.basename(new_path)}")
    
    if not os.path.exists(old_path):
        print(f"Warning: Old database {old_path} not found. Skipping.")
        return

    if os.path.exists(new_path):
        os.remove(new_path)

    old_conn = sqlite3.connect(old_path)
    new_conn = sqlite3.connect(new_path)
    
    # Initialize schema using the EXACT same SQL files used by Wright
    new_conn.executescript(schema_sql)
    # Set user_version to 1 so sqlx::migrate! knows we are starting from a clean V1
    new_conn.execute("PRAGMA user_version = 1")
    new_conn.commit()

    for table in table_list:
        common = get_common_columns(old_conn.cursor(), new_conn.cursor(), table)
        if not common:
            print(f"    Table '{table}' skipped (not found or no common fields).")
            continue
        
        cols_str = ", ".join(common)
        print(f"    Table '{table}': migrating fields [{cols_str}]")
        
        # Data copy logic
        rows = old_conn.execute(f"SELECT {cols_str} FROM {table}").fetchall()
        placeholders = ", ".join(["?" for _ in common])
        new_conn.executemany(f"INSERT INTO {table} ({cols_str}) VALUES ({placeholders})", rows)
        print(f"    Successfully migrated {len(rows)} records into '{table}'.")

    new_conn.commit()
    old_conn.close()
    new_conn.close()

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
    migrate_db(OLD_INSTALLED_DB, NEW_INSTALLED_DB, installed_schema, installed_tables)

    # 2. Migrate Archives DB (Local archive catalogue)
    archive_tables = ["parts", "dependencies", "provides", "conflicts", "replaces"]
    migrate_db(OLD_ARCHIVES_DB, NEW_ARCHIVES_DB, archive_schema, archive_tables)

    print("\n[SUCCESS] Migration complete. Both databases are now clean V1 and consistent with the new architecture.")

if __name__ == "__main__":
    main()

-- V1: Initial Archive Inventory Schema

CREATE TABLE parts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    release INTEGER NOT NULL DEFAULT 1,
    epoch INTEGER NOT NULL DEFAULT 0,
    description TEXT NOT NULL DEFAULT '',
    arch TEXT NOT NULL DEFAULT 'x86_64',
    license TEXT NOT NULL DEFAULT '',
    filename TEXT NOT NULL,
    sha256 TEXT NOT NULL DEFAULT '',
    install_size INTEGER NOT NULL DEFAULT 0,
    build_date TEXT,
    registered_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(name, version, release, epoch)
);

CREATE TABLE dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    depends_on TEXT NOT NULL,
    dep_type TEXT NOT NULL DEFAULT 'runtime',
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE TABLE provides (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE TABLE conflicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE TABLE replaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    part_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    FOREIGN KEY (part_id) REFERENCES parts(id) ON DELETE CASCADE
);

CREATE INDEX idx_inventory_part_name ON parts(name);
CREATE INDEX idx_inventory_part_filename ON parts(filename);
CREATE INDEX idx_inventory_deps_part ON dependencies(part_id);
CREATE INDEX idx_inventory_provides_name ON provides(name);

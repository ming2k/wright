use crate::error::{Result, WrightError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Symlink,
    Directory,
}

impl FileType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Directory => "dir",
        }
    }
}

impl TryFrom<&str> for FileType {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "file" => Ok(Self::File),
            "symlink" => Ok(Self::Symlink),
            "dir" => Ok(Self::Directory),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown file type: {}",
                s
            ))),
        }
    }
}

impl rusqlite::ToSql for FileType {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Dependency,
    Build,
    Manual,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dependency => "dependency",
            Self::Build => "build",
            Self::Manual => "manual",
        }
    }

    pub fn is_orphan_candidate(&self) -> bool {
        matches!(self, Self::Dependency)
    }
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for Origin {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "dependency" => Ok(Self::Dependency),
            "build" => Ok(Self::Build),
            "manual" => Ok(Self::Manual),
            _ => Err(WrightError::DatabaseError(format!("unknown origin: {}", s))),
        }
    }
}

impl rusqlite::ToSql for Origin {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepType {
    Runtime,
    Link,
    Build,
}

impl DepType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Link => "link",
            Self::Build => "build",
        }
    }
}

impl TryFrom<&str> for DepType {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "runtime" => Ok(Self::Runtime),
            "link" => Ok(Self::Link),
            "build" => Ok(Self::Build),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown dep type: {}",
                s
            ))),
        }
    }
}

impl rusqlite::ToSql for DepType {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Text(self.as_str().as_bytes()),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct InstalledPart {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub description: String,
    pub arch: String,
    pub license: String,
    pub url: Option<String>,
    pub installed_at: String,
    pub install_size: u64,
    pub part_hash: Option<String>,
    pub install_scripts: Option<String>,
    pub assumed: bool,
    pub origin: Origin,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub file_hash: Option<String>,
    pub file_type: FileType,
    pub file_mode: Option<u32>,
    pub file_size: Option<u64>,
    pub is_config: bool,
}

#[derive(Debug, Clone)]
pub struct NewPart<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: u32,
    pub epoch: u32,
    pub description: &'a str,
    pub arch: &'a str,
    pub license: &'a str,
    pub url: Option<&'a str>,
    pub install_size: u64,
    pub part_hash: Option<&'a str>,
    pub install_scripts: Option<&'a str>,
    pub origin: Origin,
}

impl<'a> Default for NewPart<'a> {
    fn default() -> Self {
        Self {
            name: "",
            version: "",
            release: 0,
            epoch: 0,
            description: "",
            arch: "",
            license: "",
            url: None,
            install_size: 0,
            part_hash: None,
            install_scripts: None,
            origin: Origin::Manual,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub constraint: Option<String>,
    pub dep_type: DepType,
}

#[derive(Debug, Clone)]
pub struct TransactionRecord {
    pub timestamp: String,
    pub operation: String,
    pub part_name: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub status: String,
}

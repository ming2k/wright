use crate::error::{Result, WrightError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum FileType {
    File,
    Symlink,
    #[sqlx(rename = "dir")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum DepType {
    Runtime,
}

impl DepType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
        }
    }
}

impl TryFrom<&str> for DepType {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "runtime" => Ok(Self::Runtime),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown dep type: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstalledPart {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: i64, // SQLite INTEGER is i64
    pub epoch: i64,
    pub description: Option<String>,
    pub arch: String,
    pub license: Option<String>,
    pub url: Option<String>,
    pub installed_at: Option<String>,
    pub install_size: Option<i64>,
    pub part_hash: Option<String>,
    pub install_scripts: Option<String>,
    pub assumed: bool,
    pub origin: Origin,
    pub plan_name: Option<String>,
    pub plan_id: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FileEntry {
    pub path: String,
    pub file_hash: Option<String>,
    pub file_type: FileType,
    pub file_mode: Option<i64>,
    pub file_size: Option<i64>,
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
    pub plan_name: Option<&'a str>,
    pub plan_id: Option<i64>,
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
            plan_name: None,
            plan_id: None,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dependency {
    #[sqlx(rename = "depends_on")]
    pub name: String,
    pub version_constraint: Option<String>,
    pub dep_type: DepType,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TransactionRecord {
    pub timestamp: Option<String>,
    pub operation: String,
    pub part_name: String,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub status: String,
}

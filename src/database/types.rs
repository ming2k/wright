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

/// How a part entered the system.
///
/// Variant order determines the upgrade priority used by `set_origin`:
/// higher variants are never silently downgraded to lower ones.
/// `External` sits above `Manual` so that `set_origin(name, Manual)` is
/// always a no-op for externally provided parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum Origin {
    Dependency,
    Forge,
    Manual,
    /// Registered with `wright assume` — provided by the host system, not built
    /// or installed by wright. Has no filesystem footprint; managed exclusively
    /// via `wright assume` / `wright unassume`.
    External,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dependency => "dependency",
            Self::Forge => "forge",
            Self::Manual => "manual",
            Self::External => "external",
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
            "forge" => Ok(Self::Forge),
            "manual" => Ok(Self::Manual),
            "external" => Ok(Self::External),
            _ => Err(WrightError::DatabaseError(format!("unknown origin: {}", s))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum HistoryAction {
    Install,
    Upgrade,
    Remove,
    Rollback,
}

impl std::fmt::Display for HistoryAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Install => "install",
            Self::Upgrade => "upgrade",
            Self::Remove => "remove",
            Self::Rollback => "rollback",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum HistoryStatus {
    Pending,
    Completed,
    Failed,
    RolledBack,
}

/// Macro-level delivery transaction state (one per user command).
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Planning,
    Ready,
    Applying,
    Completed,
    RolledBack,
}

/// Per-operation state within a delivery transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum OpStatus {
    Pending,
    Extracting,
    HooksRunning,
    Done,
    Failed,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeliveryTransaction {
    pub id: i64,
    pub command: String,
    pub status: DeliveryStatus,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TransactionOp {
    pub id: i64,
    pub transaction_id: i64,
    pub part_name: String,
    pub part_hash: String,
    pub action_type: String,
    pub execution_order: i64,
    pub status: OpStatus,
    pub old_hash: Option<String>,
    pub error_msg: Option<String>,
}

impl std::fmt::Display for HistoryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        })
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstalledPart {
    pub id: i64,
    pub name: String,
    pub plan_id: i64,
    pub installed_at: Option<String>,
    pub part_hash: Option<String>,
    pub deploy_scripts: Option<String>,
    pub origin: Origin,
}

/// Part combined with its plan metadata for display queries.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PartWithPlan {
    pub id: i64,
    pub name: String,
    pub plan_id: i64,
    pub installed_at: Option<String>,
    pub part_hash: Option<String>,
    pub deploy_scripts: Option<String>,
    pub origin: Origin,
    pub plan_name: String,
    pub version: String,
    pub release: i64,
    pub epoch: i64,
    pub arch: String,
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
    pub plan_id: i64,
    pub part_hash: Option<&'a str>,
    pub deploy_scripts: Option<&'a str>,
    pub origin: Origin,
}

impl<'a> Default for NewPart<'a> {
    fn default() -> Self {
        Self {
            name: "",
            plan_id: 0,
            part_hash: None,
            deploy_scripts: None,
            origin: Origin::Manual,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NewPlan<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub release: u32,
    pub epoch: u32,
    pub arch: &'a str,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Dependency {
    #[sqlx(rename = "depends_on")]
    pub name: String,
    pub version_constraint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionContext {
    pub id: String,
    pub command: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HistoryRecord {
    pub timestamp: Option<String>,
    pub session_id: String,
    pub command: String,
    pub part_name: String,
    pub action: HistoryAction,
    pub old_version: Option<String>,
    pub new_version: Option<String>,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
    pub status: HistoryStatus,
    pub details: Option<String>,
}

//! Persistent state value types shared by queries and transactions.

use crate::error::{Result, WrightError};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::Row;

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

impl ToSql for FileType {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for FileType {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::try_from(s).map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// How a part entered the system.
///
/// Variant order determines the upgrade priority used by `set_origin`:
/// higher variants are never silently downgraded to lower ones.
/// `External` sits above `Manual` so that `set_origin(name, Manual)` is
/// always a no-op for externally provided parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Dependency,
    Forge,
    Manual,
    /// Registered with `wright provide` — provided by the host system, not built
    /// or installed by wright. Has no filesystem footprint; managed exclusively
    /// via `wright provide` / `wright remove`.
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

impl ToSql for Origin {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for Origin {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::try_from(s).map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAction {
    Install,
    Upgrade,
    Remove,
    Rollback,
}

impl HistoryAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Upgrade => "upgrade",
            Self::Remove => "remove",
            Self::Rollback => "rollback",
        }
    }
}

impl std::fmt::Display for HistoryAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for HistoryAction {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "install" => Ok(Self::Install),
            "upgrade" => Ok(Self::Upgrade),
            "remove" => Ok(Self::Remove),
            "rollback" => Ok(Self::Rollback),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown history action: {}",
                s
            ))),
        }
    }
}

impl ToSql for HistoryAction {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for HistoryAction {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::try_from(s).map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStatus {
    Pending,
    Completed,
    Failed,
    RolledBack,
}

impl HistoryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl std::fmt::Display for HistoryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<&str> for HistoryStatus {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "rolled_back" => Ok(Self::RolledBack),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown history status: {}",
                s
            ))),
        }
    }
}

impl ToSql for HistoryStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for HistoryStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::try_from(s).map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// Macro-level delivery transaction state (one per user command).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryStatus {
    Planning,
    Ready,
    Applying,
    Completed,
    RolledBack,
}

impl DeliveryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Ready => "ready",
            Self::Applying => "applying",
            Self::Completed => "completed",
            Self::RolledBack => "rolled_back",
        }
    }
}

impl TryFrom<&str> for DeliveryStatus {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "planning" => Ok(Self::Planning),
            "ready" => Ok(Self::Ready),
            "applying" => Ok(Self::Applying),
            "completed" => Ok(Self::Completed),
            "rolled_back" => Ok(Self::RolledBack),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown delivery status: {}",
                s
            ))),
        }
    }
}

impl ToSql for DeliveryStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for DeliveryStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::try_from(s).map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// Per-operation state within a delivery transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpStatus {
    Pending,
    Extracting,
    HooksRunning,
    Done,
    Failed,
}

impl OpStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Extracting => "extracting",
            Self::HooksRunning => "hooks_running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

impl TryFrom<&str> for OpStatus {
    type Error = WrightError;

    fn try_from(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "extracting" => Ok(Self::Extracting),
            "hooks_running" => Ok(Self::HooksRunning),
            "done" => Ok(Self::Done),
            "failed" => Ok(Self::Failed),
            _ => Err(WrightError::DatabaseError(format!(
                "unknown op status: {}",
                s
            ))),
        }
    }
}

impl ToSql for OpStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

impl FromSql for OpStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::try_from(s).map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

#[derive(Debug, Clone)]
pub struct DeliveryTransaction {
    pub id: i64,
    pub command: String,
    pub status: DeliveryStatus,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl DeliveryTransaction {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            command: row.get(1)?,
            status: row.get(2)?,
            created_at: row.get(3)?,
            updated_at: row.get(4)?,
        })
    }
}

#[derive(Debug, Clone)]
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

impl TransactionOp {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            transaction_id: row.get(1)?,
            part_name: row.get(2)?,
            part_hash: row.get(3)?,
            action_type: row.get(4)?,
            execution_order: row.get(5)?,
            status: row.get(6)?,
            old_hash: row.get(7)?,
            error_msg: row.get(8)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct InstalledPart {
    pub id: i64,
    pub name: String,
    pub plan_id: i64,
    pub installed_at: Option<String>,
    pub part_hash: Option<String>,
    pub deploy_scripts: Option<String>,
    pub origin: Origin,
}

impl InstalledPart {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            plan_id: row.get(2)?,
            installed_at: row.get(3)?,
            part_hash: row.get(4)?,
            deploy_scripts: row.get(5)?,
            origin: row.get(6)?,
        })
    }
}

/// Part combined with its plan metadata for display queries.
#[derive(Debug, Clone)]
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

impl PartWithPlan {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            plan_id: row.get(2)?,
            installed_at: row.get(3)?,
            part_hash: row.get(4)?,
            deploy_scripts: row.get(5)?,
            origin: row.get(6)?,
            plan_name: row.get(7)?,
            version: row.get(8)?,
            release: row.get(9)?,
            epoch: row.get(10)?,
            arch: row.get(11)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub file_hash: Option<String>,
    pub file_type: FileType,
    pub file_mode: Option<i64>,
    pub file_size: Option<i64>,
    pub is_config: bool,
}

impl FileEntry {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            path: row.get(0)?,
            file_hash: row.get(1)?,
            file_type: row.get(2)?,
            file_mode: row.get(3)?,
            file_size: row.get(4)?,
            is_config: row.get(5)?,
        })
    }
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

/// Provenance fields persisted alongside a registered plan.
///
/// This is a persistence input rather than an archive-format type so the
/// state crate does not need to depend on the producer of the metadata.
#[derive(Debug, Clone, Copy)]
pub struct NewPlanProvenance<'a> {
    pub plan_checksum: Option<&'a str>,
    pub source_checksums: &'a [String],
    pub wright_version: &'a str,
    pub isolation: &'a str,
}

/// Complete input for inserting or refreshing a plan registry entry.
#[derive(Debug, Clone)]
pub struct RegisterPlan<'a> {
    pub plan: NewPlan<'a>,
    pub provenance: Option<NewPlanProvenance<'a>>,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub version_constraint: Option<String>,
}

impl Dependency {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            name: row.get(0)?,
            version_constraint: row.get(1)?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SessionContext {
    pub id: String,
    pub command: String,
}

#[derive(Debug, Clone)]
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

impl HistoryRecord {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            timestamp: row.get(0)?,
            session_id: row.get(1)?,
            command: row.get(2)?,
            part_name: row.get(3)?,
            action: row.get(4)?,
            old_version: row.get(5)?,
            new_version: row.get(6)?,
            old_hash: row.get(7)?,
            new_hash: row.get(8)?,
            status: row.get(9)?,
            details: row.get(10)?,
        })
    }
}

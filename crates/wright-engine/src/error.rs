#[derive(Debug, thiserror::Error)]
pub enum WrightError {
    #[error("parse error: {0}")]
    ParseError(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("forge error: {0}")]
    ForgeError(String),

    #[error("deploy error: {0}")]
    DeployError(String),

    #[error("remove error: {0}")]
    RemoveError(String),

    #[error("part error: {0}")]
    PartError(String),

    #[error("config error: {0}")]
    ConfigError(String),

    #[error("access denied: {0}. (hint: try running with sudo)")]
    AccessDenied(String),

    #[error("lock error: {0}")]
    LockError(String),

    #[error("version error: {0}")]
    VersionError(String),

    #[error("dependency error: {0}")]
    DependencyError(String),

    #[error("part not found: {0}")]
    PartNotFound(String),

    #[error("ambiguous target: {0}")]
    AmbiguousTarget(String),

    #[error("part already deployed: {0}")]
    PartAlreadyInstalled(String),

    #[error("upgrade error: {0}")]
    UpgradeError(String),

    #[error("script error: {0}")]
    ScriptError(String),

    #[error("validation error: {0}")]
    ValidationError(String),

    #[error("isolation error: {0}")]
    IsolationError(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("TOML deserialization error: {0}")]
    TomlError(#[from] toml::de::Error),

    /// Structured context wrapper: keeps the underlying error as a real
    /// `source()` so failure reports walk the actual chain instead of
    /// re-parsing flattened text. Display still nests (`"{msg}: {source}"`),
    /// so single-line logs keep the full chain. Construct via
    /// [`WrightError::context`].
    #[error("{msg}: {source}")]
    Context {
        msg: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error(transparent)]
    BatchFailures(#[from] BatchFailures),

    #[error(transparent)]
    Model(#[from] wright_model::ModelError),

    #[error(transparent)]
    Plan(#[from] wright_plan::PlanError),

    #[error(transparent)]
    Part(#[from] wright_part::PartError),

    #[error(transparent)]
    State(#[from] wright_state::StateError),
}

impl WrightError {
    /// Wrap `source` with a context message, preserving the error chain
    /// (unlike `Variant(format!("…: {}", e))`, which flattens it).
    pub fn context(
        msg: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Context {
            msg: msg.into(),
            source: source.into(),
        }
    }
}

impl From<crate::isolation::IsolationError> for WrightError {
    fn from(error: crate::isolation::IsolationError) -> Self {
        match error {
            crate::isolation::IsolationError::Cancelled => {
                Self::ForgeError("cancelled by user".into())
            }
            error => Self::IsolationError(error.to_string()),
        }
    }
}

/// A single task that failed (or panicked) while its batch ran in parallel.
///
/// Tasks within one batch have no inter-dependencies, so a failure never
/// interrupts its siblings — every failure is collected and the batch is
/// settled as a whole once no task is left running (see [`BatchFailures`]).
#[derive(Debug)]
pub struct TaskFailure {
    /// Task name as scheduled in the execution plan (e.g. `linux-lts`,
    /// `gcc:bootstrap`).
    pub task: String,
    /// True when the task panicked rather than returning an error.
    pub panicked: bool,
    /// The error the task returned (or the panic payload, rendered).
    pub error: WrightError,
}

impl TaskFailure {
    pub fn failed(task: impl Into<String>, error: WrightError) -> Self {
        Self {
            task: task.into(),
            panicked: false,
            error,
        }
    }

    pub fn panicked(task: impl Into<String>, error: WrightError) -> Self {
        Self {
            task: task.into(),
            panicked: true,
            error,
        }
    }

    /// `task 'X' failed` / `task 'X' panicked` — headline without the
    /// cause chain.
    pub fn headline(&self) -> String {
        let outcome = if self.panicked { "panicked" } else { "failed" };
        format!("task '{}' {}", self.task, outcome)
    }

    /// Flatten into the single-failure error, preserving the exact Display
    /// text produced before batch settlement existed while keeping the
    /// error chain structured for the terminal report.
    fn into_legacy_error(self) -> WrightError {
        WrightError::context(self.headline(), self.error)
    }
}

/// Every failure of one fully settled batch.
///
/// A batch is the unit of dependency parallelism: its tasks have no
/// inter-dependencies, so all of them run to completion even when some
/// fail. Once the batch settles, the collected failures abort the workflow
/// together — the next batch never starts.
#[derive(Debug)]
pub struct BatchFailures {
    /// 1-based index of the batch within the execution plan.
    pub batch_num: usize,
    /// Total number of batches in the execution plan.
    pub total_batches: usize,
    /// Every task that failed, in completion order.
    pub failures: Vec<TaskFailure>,
}

impl BatchFailures {
    /// Settle a finished batch: no failures → `Ok(())`; exactly one → the
    /// legacy flattened error so the terminal report stays byte-identical
    /// to pre-settlement output; more → the aggregated [`BatchFailures`]
    /// error.
    pub fn settle(
        batch_num: usize,
        total_batches: usize,
        mut failures: Vec<TaskFailure>,
    ) -> Result<()> {
        match failures.len() {
            0 => Ok(()),
            1 => Err(failures.pop().unwrap().into_legacy_error()),
            _ => Err(Self {
                batch_num,
                total_batches,
                failures,
            }
            .into()),
        }
    }
}

impl std::fmt::Display for BatchFailures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.failures.len();
        let plural = if count == 1 { "" } else { "s" };
        if self.total_batches > 1 {
            write!(
                f,
                "{} task{} failed in batch {}/{}",
                count, plural, self.batch_num, self.total_batches
            )
        } else {
            write!(f, "{} task{} failed", count, plural)
        }
    }
}

impl std::error::Error for BatchFailures {}

pub type Result<T> = std::result::Result<T, WrightError>;

/// Extension trait that adds `.context()` to any Result, wrapping the
/// error in a structured [`WrightError::Context`] that preserves the
/// source chain.
pub trait WrightResultExt<T> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> WrightResultExt<T>
    for std::result::Result<T, E>
{
    fn context(self, msg: impl std::fmt::Display) -> Result<T> {
        self.map_err(|e| WrightError::context(msg.to_string(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    fn failure(task: &str) -> TaskFailure {
        TaskFailure::failed(task, WrightError::ForgeError("boom".into()))
    }

    #[test]
    fn settle_with_no_failures_is_ok() {
        assert!(BatchFailures::settle(1, 3, vec![]).is_ok());
    }

    #[test]
    fn settle_with_one_failure_keeps_legacy_report() {
        let err = BatchFailures::settle(1, 3, vec![failure("bison")]).unwrap_err();
        assert_eq!(err.to_string(), "task 'bison' failed: forge error: boom");
        // …while the chain stays structured for the terminal report.
        assert!(err.source().is_some());
    }

    #[test]
    fn settle_with_one_panic_keeps_legacy_report() {
        let err = BatchFailures::settle(
            1,
            1,
            vec![TaskFailure::panicked(
                "bison",
                WrightError::ForgeError("task panicked".into()),
            )],
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "task 'bison' panicked: forge error: task panicked"
        );
    }

    #[test]
    fn settle_with_multiple_failures_aggregates() {
        let err = BatchFailures::settle(2, 3, vec![failure("a"), failure("b")]).unwrap_err();
        let WrightError::BatchFailures(batch) = err else {
            panic!("expected BatchFailures, got {err:?}");
        };
        assert_eq!(batch.batch_num, 2);
        assert_eq!(batch.total_batches, 3);
        assert_eq!(batch.failures.len(), 2);
        assert_eq!(batch.to_string(), "2 tasks failed in batch 2/3");
    }

    #[test]
    fn batch_failures_display_omits_batch_numbers_for_single_batch_plans() {
        let batch = BatchFailures {
            batch_num: 1,
            total_batches: 1,
            failures: vec![failure("a"), failure("b")],
        };
        assert_eq!(batch.to_string(), "2 tasks failed");
    }
}

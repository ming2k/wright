use crate::error::Result;
use super::types::{Dependency, FileEntry, InstalledPart, PartWithPlan};
use super::plans::PlanRecord;
use super::{InstalledDb, ReadOnlyDb};

/// Read-only domain query interface for the system registry (ADR-0043, ADR-0048).
///
/// Decouples graph resolution, inspection commands, and planners from the
/// concrete SQLite connection actor and database migrations.
pub trait RegistryQuery: Send + Sync {
    /// Retrieve a plan by its canonical name.
    fn get_plan(&self, name: &str) -> impl std::future::Future<Output = Result<Option<PlanRecord>>> + Send;

    /// Retrieve a plan by its internal database ID.
    fn get_plan_by_id(&self, id: i64) -> impl std::future::Future<Output = Result<Option<PlanRecord>>> + Send;

    /// List all currently registered plans.
    fn list_plans(&self) -> impl std::future::Future<Output = Result<Vec<PlanRecord>>> + Send;

    /// Retrieve an installed part by its name.
    fn get_part(&self, name: &str) -> impl std::future::Future<Output = Result<Option<InstalledPart>>> + Send;

    /// List all currently installed parts with their originating plan metadata.
    fn list_parts(&self) -> impl std::future::Future<Output = Result<Vec<PartWithPlan>>> + Send;

    /// Retrieve all parts belonging to a specific plan.
    fn get_parts_by_plan_id(&self, plan_id: i64) -> impl std::future::Future<Output = Result<Vec<InstalledPart>>> + Send;

    /// Retrieve all runtime dependencies declared for an installed part.
    fn get_dependencies(&self, part_id: i64) -> impl std::future::Future<Output = Result<Vec<Dependency>>> + Send;

    /// Retrieve all file paths owned by an installed part.
    fn get_files(&self, part_id: i64) -> impl std::future::Future<Output = Result<Vec<FileEntry>>> + Send;
}

impl RegistryQuery for ReadOnlyDb {
    fn get_plan(&self, name: &str) -> impl std::future::Future<Output = Result<Option<PlanRecord>>> + Send {
        ReadOnlyDb::get_plan(self, name)
    }

    fn get_plan_by_id(&self, id: i64) -> impl std::future::Future<Output = Result<Option<PlanRecord>>> + Send {
        ReadOnlyDb::get_plan_by_id(self, id)
    }

    fn list_plans(&self) -> impl std::future::Future<Output = Result<Vec<PlanRecord>>> + Send {
        ReadOnlyDb::list_plans(self)
    }

    fn get_part(&self, name: &str) -> impl std::future::Future<Output = Result<Option<InstalledPart>>> + Send {
        ReadOnlyDb::get_part(self, name)
    }

    fn list_parts(&self) -> impl std::future::Future<Output = Result<Vec<PartWithPlan>>> + Send {
        ReadOnlyDb::list_parts(self)
    }

    fn get_parts_by_plan_id(&self, plan_id: i64) -> impl std::future::Future<Output = Result<Vec<InstalledPart>>> + Send {
        ReadOnlyDb::get_parts_by_plan_id(self, plan_id)
    }

    fn get_dependencies(&self, part_id: i64) -> impl std::future::Future<Output = Result<Vec<Dependency>>> + Send {
        ReadOnlyDb::get_dependencies(self, part_id)
    }

    fn get_files(&self, part_id: i64) -> impl std::future::Future<Output = Result<Vec<FileEntry>>> + Send {
        ReadOnlyDb::get_files(self, part_id)
    }
}

impl RegistryQuery for InstalledDb {
    fn get_plan(&self, name: &str) -> impl std::future::Future<Output = Result<Option<PlanRecord>>> + Send {
        (**self).get_plan(name)
    }

    fn get_plan_by_id(&self, id: i64) -> impl std::future::Future<Output = Result<Option<PlanRecord>>> + Send {
        (**self).get_plan_by_id(id)
    }

    fn list_plans(&self) -> impl std::future::Future<Output = Result<Vec<PlanRecord>>> + Send {
        (**self).list_plans()
    }

    fn get_part(&self, name: &str) -> impl std::future::Future<Output = Result<Option<InstalledPart>>> + Send {
        (**self).get_part(name)
    }

    fn list_parts(&self) -> impl std::future::Future<Output = Result<Vec<PartWithPlan>>> + Send {
        (**self).list_parts()
    }

    fn get_parts_by_plan_id(&self, plan_id: i64) -> impl std::future::Future<Output = Result<Vec<InstalledPart>>> + Send {
        (**self).get_parts_by_plan_id(plan_id)
    }

    fn get_dependencies(&self, part_id: i64) -> impl std::future::Future<Output = Result<Vec<Dependency>>> + Send {
        (**self).get_dependencies(part_id)
    }

    fn get_files(&self, part_id: i64) -> impl std::future::Future<Output = Result<Vec<FileEntry>>> + Send {
        (**self).get_files(part_id)
    }
}

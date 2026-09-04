use uuid::Uuid;

use crate::core::{Ecosystem, PackageRecord, PackageTask};

/// Commands accepted by an ecosystem worker.
#[derive(Debug, Clone)]
pub enum WorkerCommand {
    Scan(Ecosystem),
    Update(PackageTask),
    Uninstall(PackageTask),
    RefreshDiskUsage(PackageRecord),
    Shutdown,
}

impl WorkerCommand {
    pub(crate) fn ecosystem(&self) -> Option<Ecosystem> {
        match self {
            Self::Scan(ecosystem) => Some(*ecosystem),
            Self::RefreshDiskUsage(package) => Some(package.ecosystem),
            Self::Update(task) | Self::Uninstall(task) => Some(task.ecosystem),
            Self::Shutdown => None,
        }
    }

    pub(crate) fn task_id(&self) -> Option<Uuid> {
        match self {
            Self::Update(task) | Self::Uninstall(task) => Some(task.task_id),
            Self::Scan(_) | Self::RefreshDiskUsage(_) | Self::Shutdown => None,
        }
    }
}

use std::sync::{Arc, Mutex};

use crate::core::{Ecosystem, PackageRecord};
use crate::persistence::{Database, DiskUsageCacheEntry};

use super::{DiskUsageError, Result};

#[derive(Clone)]
pub struct CacheStore {
    backend: Arc<CacheBackend>,
}

enum CacheBackend {
    Memory(Mutex<Vec<DiskUsageCacheEntry>>),
    Database(Arc<Database>),
}

impl CacheStore {
    pub fn in_memory() -> Self {
        Self {
            backend: Arc::new(CacheBackend::Memory(Mutex::new(Vec::new()))),
        }
    }

    pub fn from_database(database: Arc<Database>) -> Self {
        Self {
            backend: Arc::new(CacheBackend::Database(database)),
        }
    }

    pub fn put(&self, entry: DiskUsageCacheEntry) -> Result<()> {
        match self.backend.as_ref() {
            CacheBackend::Memory(entries) => {
                let mut entries = entries
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                entries.retain(|cached| !same_key(cached, &entry));
                entries.push(entry);
                Ok(())
            }
            CacheBackend::Database(database) => {
                database.upsert_disk_usage(&entry).map_err(Into::into)
            }
        }
    }

    pub fn get(&self, package: &PackageRecord) -> Result<Option<DiskUsageCacheEntry>> {
        match self.backend.as_ref() {
            CacheBackend::Memory(entries) => Ok(entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .find(|entry| entry_matches_package(entry, package))
                .cloned()),
            CacheBackend::Database(database) => database
                .find_disk_usage_for_package(
                    ecosystem_name(package.ecosystem),
                    &package.id,
                    installed_version(package),
                )
                .map_err(Into::into),
        }
    }

    pub fn get_for(
        &self,
        package: &PackageRecord,
        install_root: &str,
    ) -> Result<Option<DiskUsageCacheEntry>> {
        match self.backend.as_ref() {
            CacheBackend::Memory(entries) => Ok(entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .find(|entry| {
                    entry_matches_package(entry, package) && entry.install_root == install_root
                })
                .cloned()),
            CacheBackend::Database(database) => database
                .find_disk_usage(
                    ecosystem_name(package.ecosystem),
                    &package.id,
                    installed_version(package),
                    install_root,
                )
                .map_err(Into::into),
        }
    }

    pub fn invalidate_after_success(&self, package: &PackageRecord) -> Result<()> {
        self.remove_package(package.ecosystem, &package.id)
    }

    pub fn remove_package(&self, ecosystem: Ecosystem, package_id: &str) -> Result<()> {
        match self.backend.as_ref() {
            CacheBackend::Memory(entries) => {
                entries
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .retain(|entry| {
                        entry.ecosystem != ecosystem_name(ecosystem)
                            || entry.package_id != package_id
                    });
                Ok(())
            }
            CacheBackend::Database(database) => database
                .delete_disk_usage(ecosystem, package_id)
                .map(|_| ())
                .map_err(Into::into),
        }
    }
}

pub(crate) fn ecosystem_name(ecosystem: Ecosystem) -> &'static str {
    match ecosystem {
        Ecosystem::Homebrew => "homebrew",
        Ecosystem::Npm => "npm",
        Ecosystem::Pip => "pip",
        Ecosystem::Gem => "gem",
        Ecosystem::Rustup => "rustup",
    }
}

pub(crate) fn installed_version(package: &PackageRecord) -> &str {
    package.current_version.as_deref().unwrap_or_default()
}

fn entry_matches_package(entry: &DiskUsageCacheEntry, package: &PackageRecord) -> bool {
    entry.ecosystem == ecosystem_name(package.ecosystem)
        && entry.package_id == package.id
        && entry.installed_version == installed_version(package)
}

fn same_key(left: &DiskUsageCacheEntry, right: &DiskUsageCacheEntry) -> bool {
    left.ecosystem == right.ecosystem
        && left.package_id == right.package_id
        && left.installed_version == right.installed_version
        && left.install_root == right.install_root
}

impl From<crate::persistence::PersistenceError> for DiskUsageError {
    fn from(value: crate::persistence::PersistenceError) -> Self {
        Self::Persistence(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::adapters::package_record;
    use crate::core::{Ecosystem, ResourceKind};
    use crate::persistence::DiskUsageCacheEntry;

    use super::*;

    fn package(name: &str) -> PackageRecord {
        package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            name,
            Some("1.0.0".into()),
            None,
        )
    }

    fn cache_entry(package: &PackageRecord, bytes: u64) -> DiskUsageCacheEntry {
        DiskUsageCacheEntry {
            ecosystem: "npm".into(),
            package_id: package.id.clone(),
            installed_version: "1.0.0".into(),
            install_root: "/global".into(),
            path_signature: "signature".into(),
            bytes,
            status: crate::core::DiskUsageStatus::Ready,
            scanned_at: 10,
        }
    }

    #[test]
    fn successful_update_invalidates_only_updated_package() {
        let cache = CacheStore::in_memory();
        let a = package("a");
        let b = package("b");
        cache.put(cache_entry(&a, 100)).unwrap();
        cache.put(cache_entry(&b, 200)).unwrap();

        cache.invalidate_after_success(&a).unwrap();

        assert!(cache.get(&a).unwrap().is_none());
        assert_eq!(cache.get(&b).unwrap().unwrap().bytes, 200);
    }
}

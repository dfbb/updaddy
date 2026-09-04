use rusqlite::params;

use super::db::{Database, Result};

impl Database {
    pub fn cleanup_before(&self, cutoff: i64) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let mut removed = 0u64;
        removed += tx.execute(
            "DELETE FROM log_entries WHERE emitted_at < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute(
            "DELETE FROM task_attempts WHERE finished_at IS NOT NULL AND finished_at < ?1",
            params![cutoff],
        )? as u64;
        removed += tx.execute("DELETE FROM package_tasks WHERE status IN ('succeeded','failed','cancelled','interrupted') AND batch_id IN (SELECT batch_id FROM operation_batches WHERE created_at < ?1)", params![cutoff])? as u64;
        removed += tx.execute("DELETE FROM operation_batches WHERE created_at < ?1 AND batch_id NOT IN (SELECT batch_id FROM package_tasks)", params![cutoff])? as u64;
        tx.commit()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::LogEntry;
    use crate::persistence::{Database, DiskUsageCacheEntry};

    #[test]
    fn disk_usage_cache_is_reused_for_same_package_version_and_root() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("updaddy.sqlite")).unwrap();
        let entry = DiskUsageCacheEntry::ready("npm", "eslint", "9.0.0", "/global", 4096);
        db.upsert_disk_usage(&entry).unwrap();
        assert_eq!(
            db.find_disk_usage("npm", "eslint", "9.0.0", "/global")
                .unwrap()
                .unwrap()
                .bytes,
            4096
        );
    }

    #[test]
    fn cleanup_removes_records_older_than_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("updaddy.sqlite")).unwrap();
        db.append_log(&LogEntry::at("old", 1_600_000_000)).unwrap();
        assert_eq!(db.cleanup_before(1_700_000_000).unwrap(), 1);
    }
}

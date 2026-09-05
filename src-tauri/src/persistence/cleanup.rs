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
        removed += tx.execute("DELETE FROM task_attempts WHERE task_id IN (SELECT task_id FROM package_tasks WHERE batch_id IN (SELECT batch_id FROM operation_batches WHERE created_at < ?1))", params![cutoff])? as u64;
        removed += tx.execute("DELETE FROM package_tasks WHERE batch_id IN (SELECT batch_id FROM operation_batches WHERE created_at < ?1)", params![cutoff])? as u64;
        removed += tx.execute(
            "DELETE FROM operation_batches WHERE created_at < ?1",
            params![cutoff],
        )? as u64;
        tx.commit()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn cleanup_removes_old_batch_with_pending_and_running_tasks() {
        use crate::core::{Ecosystem, Operation, OperationBatch, PackageTask, TaskStatus};
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("updaddy.sqlite")).unwrap();
        let pending = PackageTask::new(Ecosystem::Npm, "pending", Operation::Update);
        let mut running = PackageTask::new(Ecosystem::Npm, "running", Operation::Update);
        running.status = TaskStatus::Running;
        let batch = OperationBatch {
            batch_id: uuid::Uuid::new_v4(),
            ecosystem: Ecosystem::Npm,
            tasks: vec![pending, running],
            created_at: 1_600_000_000,
        };
        db.create_batch(&batch).unwrap();
        assert_eq!(db.cleanup_before(1_700_000_000).unwrap(), 3);
        assert!(db.load_batch(batch.batch_id).unwrap().is_none());
    }

    #[test]
    fn cleanup_removes_expired_attempt_from_recent_batch() {
        use crate::core::{Ecosystem, Operation, OperationBatch, PackageTask};
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("updaddy.sqlite")).unwrap();
        let task = PackageTask::new(Ecosystem::Npm, "recent", Operation::Update);
        let task_id = task.task_id;
        let batch = OperationBatch {
            batch_id: uuid::Uuid::new_v4(),
            ecosystem: Ecosystem::Npm,
            tasks: vec![task],
            created_at: 1_800_000_000,
        };
        db.create_batch(&batch).unwrap();
        db.record_task_attempt(task_id, 1, "succeeded", Some(1), Some(1_600_000_000))
            .unwrap();
        assert_eq!(db.cleanup_before(1_700_000_000).unwrap(), 1);
        assert!(db.list_task_attempts(task_id).unwrap().is_empty());
        assert!(db.load_batch(batch.batch_id).unwrap().is_some());
    }
}

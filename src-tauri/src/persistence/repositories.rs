use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json;
use uuid::Uuid;

use crate::core::{
    DiskUsageStatus, Ecosystem, LogEntry, OperationBatch, PackageRecord, PackageTask,
};

use super::db::{Database, PersistenceError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskUsageCacheEntry {
    pub ecosystem: String,
    pub package_id: String,
    pub installed_version: String,
    pub install_root: String,
    pub path_signature: String,
    pub bytes: u64,
    pub status: DiskUsageStatus,
    pub scanned_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAttempt {
    pub id: i64,
    pub attempt: u32,
    pub status: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

impl DiskUsageCacheEntry {
    pub fn ready(
        ecosystem: impl Into<String>,
        package_id: impl Into<String>,
        version: impl Into<String>,
        root: impl Into<String>,
        bytes: u64,
    ) -> Self {
        Self {
            ecosystem: ecosystem.into(),
            package_id: package_id.into(),
            installed_version: version.into(),
            install_root: root.into(),
            path_signature: String::new(),
            bytes,
            status: DiskUsageStatus::Ready,
            scanned_at: chrono::Utc::now().timestamp(),
        }
    }
}

fn enum_name<T: serde::Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_value(v)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str, field: &str) -> Result<T> {
    serde_json::from_str(&format!("\"{value}\""))
        .map_err(|_| PersistenceError::InvalidValue(format!("unknown {field}: {value}")))
}

impl Database {
    pub fn save_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO settings(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?;
        Ok(())
    }

    pub fn load_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM settings WHERE key=?1",
            params![key],
            |r| r.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn list_tasks(&self) -> Result<Vec<PackageTask>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT task_id,ecosystem,name,operation,status,error FROM package_tasks ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |row| {
            let task_id = row
                .get::<_, String>(0)?
                .parse::<Uuid>()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let ecosystem = parse_enum(&row.get::<_, String>(1)?, "ecosystem")
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let operation = parse_enum(&row.get::<_, String>(3)?, "operation")
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let status = parse_enum(&row.get::<_, String>(4)?, "task status")
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let error = row
                .get::<_, Option<String>>(5)?
                .map(|v| parse_enum(&v, "task error"))
                .transpose()
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(PackageTask {
                task_id,
                ecosystem,
                name: row.get(2)?,
                operation,
                status,
                error,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    pub fn list_snapshots(&self) -> Result<Vec<PackageRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT payload_json FROM package_snapshots ORDER BY ecosystem, id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            let payload = row?;
            serde_json::from_str(&payload).map_err(PersistenceError::from)
        })
        .collect()
    }

    /// 保存计划状态；键和值均由 scheduler 负责定义，便于未来扩展而无需改表结构。
    pub fn save_scheduler_state(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO scheduler_state(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn load_scheduler_state(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM scheduler_state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn save_snapshot(&self, snapshot: &PackageRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute("INSERT INTO package_snapshots(id,ecosystem,payload_json,updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET ecosystem=excluded.ecosystem,payload_json=excluded.payload_json,updated_at=excluded.updated_at", params![snapshot.id, enum_name(&snapshot.ecosystem)?, serde_json::to_string(snapshot)?, chrono::Utc::now().timestamp()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn load_snapshot(&self, id: &str) -> Result<Option<PackageRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT payload_json FROM package_snapshots WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        rows.next()?
            .map(|row| Ok(serde_json::from_str(&row.get::<_, String>(0)?)?))
            .transpose()
    }

    pub fn find_snapshot(&self, ecosystem: Ecosystem, name: &str) -> Result<Option<PackageRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT payload_json FROM package_snapshots WHERE ecosystem = ?1 ORDER BY id",
        )?;
        let snapshots = stmt.query_map(params![enum_name(&ecosystem)?], |row| {
            row.get::<_, String>(0)
        })?;
        for snapshot in snapshots {
            let snapshot: PackageRecord = serde_json::from_str(&snapshot?)?;
            if snapshot.name == name {
                return Ok(Some(snapshot));
            }
        }
        Ok(None)
    }

    pub fn reconcile_snapshots(
        &self,
        ecosystem: Ecosystem,
        retained_ids: &[String],
    ) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let ecosystem_name = enum_name(&ecosystem)?;
        let mut stmt = tx.prepare("SELECT id FROM package_snapshots WHERE ecosystem = ?1")?;
        let existing = stmt
            .query_map(params![ecosystem_name], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        let stale = existing
            .into_iter()
            .filter(|id| !retained_ids.iter().any(|retained| retained == id))
            .collect::<Vec<_>>();
        for id in &stale {
            tx.execute(
                "DELETE FROM disk_usage_cache WHERE ecosystem = ?1 AND package_id = ?2",
                params![ecosystem_name, id],
            )?;
            tx.execute("DELETE FROM package_snapshots WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(stale.len() as u64)
    }

    pub fn upsert_disk_usage(&self, entry: &DiskUsageCacheEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let bytes = i64::try_from(entry.bytes).map_err(|_| {
            PersistenceError::InvalidValue("disk usage bytes exceed SQLite integer range".into())
        })?;
        tx.execute("INSERT INTO disk_usage_cache(ecosystem,package_id,installed_version,install_root,path_signature,bytes,status,scanned_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(ecosystem,package_id,installed_version,install_root) DO UPDATE SET path_signature=excluded.path_signature,bytes=excluded.bytes,status=excluded.status,scanned_at=excluded.scanned_at", params![entry.ecosystem, entry.package_id, entry.installed_version, entry.install_root, entry.path_signature, bytes, enum_name(&entry.status)?, entry.scanned_at])?;
        tx.commit()?;
        Ok(())
    }

    pub fn find_disk_usage(
        &self,
        ecosystem: &str,
        package_id: &str,
        version: &str,
        root: &str,
    ) -> Result<Option<DiskUsageCacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT ecosystem,package_id,installed_version,install_root,path_signature,bytes,status,scanned_at FROM disk_usage_cache WHERE ecosystem=?1 AND package_id=?2 AND installed_version=?3 AND install_root=?4")?;
        let mut rows = stmt.query(params![ecosystem, package_id, version, root])?;
        rows.next()?.map(read_disk_usage_entry).transpose()
    }

    pub fn find_disk_usage_for_package(
        &self,
        ecosystem: &str,
        package_id: &str,
        version: &str,
    ) -> Result<Option<DiskUsageCacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT ecosystem,package_id,installed_version,install_root,path_signature,bytes,status,scanned_at FROM disk_usage_cache WHERE ecosystem=?1 AND package_id=?2 AND installed_version=?3 ORDER BY scanned_at DESC LIMIT 1")?;
        let mut rows = stmt.query(params![ecosystem, package_id, version])?;
        rows.next()?.map(read_disk_usage_entry).transpose()
    }

    pub fn delete_disk_usage(&self, ecosystem: Ecosystem, package_id: &str) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "DELETE FROM disk_usage_cache WHERE ecosystem = ?1 AND package_id = ?2",
            params![enum_name(&ecosystem)?, package_id],
        )? as u64;
        tx.commit()?;
        Ok(changed)
    }

    pub fn delete_snapshot_and_disk_usage(
        &self,
        ecosystem: Ecosystem,
        package_id: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM disk_usage_cache WHERE ecosystem = ?1 AND package_id = ?2",
            params![enum_name(&ecosystem)?, package_id],
        )?;
        tx.execute(
            "DELETE FROM package_snapshots WHERE id = ?1",
            params![package_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn create_batch(&self, batch: &OperationBatch) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO operation_batches(batch_id,ecosystem,created_at) VALUES (?1,?2,?3)",
            params![
                batch.batch_id.to_string(),
                enum_name(&batch.ecosystem)?,
                batch.created_at
            ],
        )?;
        for task in &batch.tasks {
            insert_task(&tx, batch.batch_id, task)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn append_tasks_to_task_batch(
        &self,
        parent_task_id: Uuid,
        tasks: &[PackageTask],
    ) -> Result<Option<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let batch_id = tx
            .query_row(
                "SELECT batch_id FROM package_tasks WHERE task_id = ?1",
                params![parent_task_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(batch_id) = batch_id else {
            return Ok(None);
        };
        let batch_id = batch_id.parse::<Uuid>().map_err(|_| {
            PersistenceError::InvalidValue("package task has an invalid batch id".into())
        })?;
        for task in tasks {
            insert_task(&tx, batch_id, task)?;
        }
        tx.commit()?;
        Ok(Some(batch_id))
    }

    pub fn batch_id_for_task(&self, task_id: Uuid) -> Result<Option<Uuid>> {
        let conn = self.conn.lock().unwrap();
        let batch_id = conn
            .query_row(
                "SELECT batch_id FROM package_tasks WHERE task_id = ?1",
                params![task_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        batch_id
            .map(|value| {
                value.parse::<Uuid>().map_err(|_| {
                    PersistenceError::InvalidValue("package task has an invalid batch id".into())
                })
            })
            .transpose()
    }

    pub fn append_log(&self, entry: &LogEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO log_entries(message,emitted_at,stream) VALUES (?1,?2,?3)",
            params![entry.message, entry.emitted_at, entry.stream],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn mark_running_tasks_interrupted(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE package_tasks SET status='interrupted' WHERE status='running'",
            [],
        )? as u64;
        tx.commit()?;
        Ok(changed)
    }

    pub fn set_worker_state(
        &self,
        ecosystem: Ecosystem,
        state: &str,
        updated_at: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute("INSERT INTO worker_state(ecosystem,state,updated_at) VALUES (?1,?2,?3) ON CONFLICT(ecosystem) DO UPDATE SET state=excluded.state,updated_at=excluded.updated_at", params![enum_name(&ecosystem)?, state, updated_at])?;
        tx.commit()?;
        Ok(())
    }

    pub fn worker_state(&self, ecosystem: Ecosystem) -> Result<Option<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT state, updated_at FROM worker_state WHERE ecosystem = ?1")?;
        let mut rows = stmt.query(params![enum_name(&ecosystem)?])?;
        rows.next()?
            .map(|row| Ok((row.get(0)?, row.get(1)?)))
            .transpose()
    }

    pub fn load_batch(&self, batch_id: Uuid) -> Result<Option<OperationBatch>> {
        let conn = self.conn.lock().unwrap();
        let mut batch_stmt = conn
            .prepare("SELECT ecosystem, created_at FROM operation_batches WHERE batch_id = ?1")?;
        let mut batch_rows = batch_stmt.query(params![batch_id.to_string()])?;
        let Some(batch_row) = batch_rows.next()? else {
            return Ok(None);
        };
        let ecosystem: Ecosystem = parse_enum(&batch_row.get::<_, String>(0)?, "ecosystem")?;
        let created_at = batch_row.get(1)?;
        let mut task_stmt = conn.prepare("SELECT task_id, ecosystem, name, operation, status, error FROM package_tasks WHERE batch_id = ?1 ORDER BY rowid")?;
        let tasks = task_stmt
            .query_map(params![batch_id.to_string()], |row| {
                let task_id = row
                    .get::<_, String>(0)?
                    .parse::<Uuid>()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let ecosystem = parse_enum(&row.get::<_, String>(1)?, "ecosystem")
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let operation = parse_enum(&row.get::<_, String>(3)?, "operation")
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let status = parse_enum(&row.get::<_, String>(4)?, "task status")
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let error = row
                    .get::<_, Option<String>>(5)?
                    .map(|value| parse_enum(&value, "task error"))
                    .transpose()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok(PackageTask {
                    task_id,
                    ecosystem,
                    name: row.get(2)?,
                    operation,
                    status,
                    error,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Some(OperationBatch {
            batch_id,
            ecosystem,
            tasks,
            created_at,
        }))
    }

    pub fn list_batches(&self) -> Result<Vec<OperationBatch>> {
        let batch_ids = {
            let conn = self.conn.lock().unwrap();
            let mut statement = conn.prepare(
                "SELECT batch_id FROM operation_batches ORDER BY created_at DESC, rowid DESC",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ids
        };
        batch_ids
            .into_iter()
            .map(|batch_id| {
                let batch_id = batch_id.parse::<Uuid>().map_err(|_| {
                    PersistenceError::InvalidValue("operation batch has an invalid id".into())
                })?;
                self.load_batch(batch_id)?.ok_or_else(|| {
                    PersistenceError::InvalidValue("operation batch disappeared".into())
                })
            })
            .collect()
    }

    pub fn update_task(
        &self,
        task_id: Uuid,
        status: crate::core::TaskStatus,
        error: Option<crate::core::TaskErrorKind>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let changed = tx.execute(
            "UPDATE package_tasks SET status = ?1, error = ?2 WHERE task_id = ?3",
            params![
                enum_name(&status)?,
                error.as_ref().map(enum_name).transpose()?,
                task_id.to_string()
            ],
        )? > 0;
        tx.commit()?;
        Ok(changed)
    }

    pub fn record_task_attempt(
        &self,
        task_id: Uuid,
        attempt: u32,
        status: &str,
        started_at: Option<i64>,
        finished_at: Option<i64>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute("INSERT INTO task_attempts(task_id, attempt, status, started_at, finished_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![task_id.to_string(), attempt, status, started_at, finished_at])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }

    pub fn list_task_attempts(&self, task_id: Uuid) -> Result<Vec<TaskAttempt>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, attempt, status, started_at, finished_at FROM task_attempts WHERE task_id = ?1 ORDER BY attempt, id")?;
        let attempts = stmt
            .query_map(params![task_id.to_string()], |row| {
                Ok(TaskAttempt {
                    id: row.get(0)?,
                    attempt: row.get(1)?,
                    status: row.get(2)?,
                    started_at: row.get(3)?,
                    finished_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(attempts)
    }

    pub fn list_logs(&self) -> Result<Vec<LogEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT message, emitted_at, stream FROM log_entries ORDER BY id")?;
        let entries = stmt
            .query_map([], |row| {
                Ok(LogEntry {
                    message: row.get(0)?,
                    emitted_at: row.get(1)?,
                    stream: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }
}

fn insert_task(tx: &rusqlite::Transaction<'_>, batch_id: Uuid, task: &PackageTask) -> Result<()> {
    tx.execute("INSERT INTO package_tasks(task_id,batch_id,ecosystem,name,operation,status,error) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![task.task_id.to_string(), batch_id.to_string(), enum_name(&task.ecosystem)?, task.name, enum_name(&task.operation)?, enum_name(&task.status)?, task.error.as_ref().map(enum_name).transpose()?])?;
    Ok(())
}

fn read_disk_usage_entry(row: &rusqlite::Row<'_>) -> Result<DiskUsageCacheEntry> {
    Ok(DiskUsageCacheEntry {
        ecosystem: row.get(0)?,
        package_id: row.get(1)?,
        installed_version: row.get(2)?,
        install_root: row.get(3)?,
        path_signature: row.get(4)?,
        bytes: u64::try_from(row.get::<_, i64>(5)?).map_err(|_| {
            PersistenceError::InvalidValue("disk usage bytes cannot be negative".into())
        })?,
        status: parse_status(&row.get::<_, String>(6)?)?,
        scanned_at: row.get(7)?,
    })
}

fn parse_status(s: &str) -> Result<DiskUsageStatus> {
    match s {
        "ready" => Ok(DiskUsageStatus::Ready),
        "measuring" => Ok(DiskUsageStatus::Measuring),
        "unavailable" => Ok(DiskUsageStatus::Unavailable),
        _ => Err(PersistenceError::InvalidValue(format!(
            "unknown disk usage status: {s}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Ecosystem, Operation, OperationBatch, PackageTask};

    #[test]
    fn task_attempts_can_be_read_after_recording() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("updaddy.sqlite")).unwrap();
        let task = PackageTask::new(Ecosystem::Npm, "eslint", Operation::Update);
        let task_id = task.task_id;
        let batch = OperationBatch {
            batch_id: Uuid::new_v4(),
            ecosystem: Ecosystem::Npm,
            tasks: vec![task],
            created_at: 1_700_000_000,
        };
        db.create_batch(&batch).unwrap();
        db.record_task_attempt(task_id, 1, "failed", Some(10), Some(20))
            .unwrap();
        let attempts = db.list_task_attempts(task_id).unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].finished_at, Some(20));
    }

    #[test]
    fn batches_are_listed_newest_first_with_their_tasks() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("history.sqlite")).unwrap();
        let first = OperationBatch {
            batch_id: Uuid::new_v4(),
            ecosystem: Ecosystem::Npm,
            tasks: vec![PackageTask::new(Ecosystem::Npm, "first", Operation::Scan)],
            created_at: 1,
        };
        let second = OperationBatch {
            batch_id: Uuid::new_v4(),
            ecosystem: Ecosystem::Gem,
            tasks: vec![PackageTask::new(
                Ecosystem::Gem,
                "second",
                Operation::Update,
            )],
            created_at: 2,
        };
        database.create_batch(&first).unwrap();
        database.create_batch(&second).unwrap();

        let batches = database.list_batches().unwrap();
        assert_eq!(batches[0], second);
        assert_eq!(batches[1], first);
    }
}

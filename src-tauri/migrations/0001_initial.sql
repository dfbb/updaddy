PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS worker_state (ecosystem TEXT PRIMARY KEY, state TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS manager_profiles (id TEXT PRIMARY KEY, ecosystem TEXT NOT NULL, config_json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS package_snapshots (id TEXT PRIMARY KEY, ecosystem TEXT NOT NULL, payload_json TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS disk_usage_cache (
  ecosystem TEXT NOT NULL, package_id TEXT NOT NULL, installed_version TEXT NOT NULL,
  install_root TEXT NOT NULL, path_signature TEXT NOT NULL DEFAULT '', bytes INTEGER NOT NULL,
  status TEXT NOT NULL, scanned_at INTEGER NOT NULL,
  PRIMARY KEY (ecosystem, package_id, installed_version, install_root)
);
CREATE TABLE IF NOT EXISTS operation_batches (batch_id TEXT PRIMARY KEY, ecosystem TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS package_tasks (
  task_id TEXT PRIMARY KEY, batch_id TEXT NOT NULL, ecosystem TEXT NOT NULL, name TEXT NOT NULL,
  operation TEXT NOT NULL, status TEXT NOT NULL, error TEXT,
  FOREIGN KEY(batch_id) REFERENCES operation_batches(batch_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS task_attempts (
  id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL, attempt INTEGER NOT NULL,
  status TEXT NOT NULL, started_at INTEGER, finished_at INTEGER,
  FOREIGN KEY(task_id) REFERENCES package_tasks(task_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS log_entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT, message TEXT NOT NULL, emitted_at INTEGER NOT NULL,
  stream TEXT NOT NULL, task_id TEXT,
  FOREIGN KEY(task_id) REFERENCES package_tasks(task_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS scheduler_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_log_entries_emitted_at ON log_entries(emitted_at);
CREATE INDEX IF NOT EXISTS idx_batches_created_at ON operation_batches(created_at);

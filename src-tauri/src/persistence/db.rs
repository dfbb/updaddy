use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

use super::migrations;

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid persisted value: {0}")]
    InvalidValue(String),
}

pub type Result<T> = std::result::Result<T, PersistenceError>;

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
            set_mode(parent, 0o700)?;
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn = Connection::open_with_flags(&path, flags)?;
        set_mode(&path, 0o600)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::run(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library/Application Support/updaddy/updaddy.sqlite")
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("updaddy/updaddy.sqlite")
        }
    }

    pub fn open_default() -> Result<Self> {
        Self::open(Self::default_path())
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Database;

    #[test]
    fn open_accepts_path_without_parent_directory() {
        let file_name = format!("updaddy-persistence-{}.sqlite", std::process::id());
        let path = std::path::PathBuf::from(&file_name);
        let db = Database::open(&path).expect("database path without parent should work");
        drop(db);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{file_name}-wal"));
        let _ = std::fs::remove_file(format!("{file_name}-shm"));
    }
}

use rusqlite::Connection;

use super::db::Result;

pub fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../../migrations/0001_initial.sql"))?;
    let has_task_id = conn
        .prepare("PRAGMA table_info(log_entries)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "task_id");
    if !has_task_id {
        conn.execute("ALTER TABLE log_entries ADD COLUMN task_id TEXT", [])?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_log_entries_task_id ON log_entries(task_id)",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_task_id_to_an_existing_log_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE log_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                message TEXT NOT NULL,
                emitted_at INTEGER NOT NULL,
                stream TEXT NOT NULL
            );",
        )
        .unwrap();

        run(&conn).unwrap();

        let columns = conn
            .prepare("PRAGMA table_info(log_entries)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "task_id"));
    }
}

use rusqlite::Connection;

use super::db::Result;

pub fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../../migrations/0001_initial.sql"))?;
    Ok(())
}

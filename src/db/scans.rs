use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Scan {
    pub id: i64,
    pub repository_id: i64,
    pub base_ref: String,
    pub base_sha: String,
    pub merge_base_sha: String,
    pub head_sha: String,
    pub max_depth: u32,
    pub since_date: String,
}

impl Scan {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        // Saved after the scan finishes, so we write the terminal status directly.
        conn.execute(
            "insert into scans (
                id, repository_id, base_ref, base_sha, merge_base_sha, head_sha,
                max_depth, since_date, status, finished_at
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'complete', current_timestamp)",
            params![
                self.id,
                self.repository_id,
                self.base_ref,
                self.base_sha,
                self.merge_base_sha,
                self.head_sha,
                self.max_depth as i64,
                self.since_date,
            ],
        )?;
        Ok(())
    }
}

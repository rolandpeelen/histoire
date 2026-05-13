use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Repository {
    pub id: i64,
    pub worktree_path: String,
    pub git_dir_path: String,
    pub remote_url: Option<String>,
}

impl Repository {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "insert into repositories (id, worktree_path, git_dir_path, remote_url)
             values (?1, ?2, ?3, ?4)",
            params![
                self.id,
                self.worktree_path,
                self.git_dir_path,
                self.remote_url
            ],
        )?;
        Ok(())
    }
}

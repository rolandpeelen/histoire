use anyhow::Result;
use rusqlite::{Connection, params};

pub fn ensure_repository(
    conn: &Connection,
    worktree_path: &str,
    git_dir_path: &str,
    remote_url: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "insert or ignore into repositories (worktree_path, git_dir_path, remote_url)
         values (?1, ?2, ?3)",
        params![worktree_path, git_dir_path, remote_url],
    )?;
    let id: i64 = conn.query_row(
        "select id from repositories where git_dir_path = ?1",
        params![git_dir_path],
        |row| row.get(0),
    )?;
    Ok(id)
}

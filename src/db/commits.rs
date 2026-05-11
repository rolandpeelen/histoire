use anyhow::Result;
use rusqlite::{Connection, params};

/// Fields needed to insert a `commits` row.
pub struct UpsertCommit<'a> {
    pub repository_id: i64,
    pub sha: &'a str,
    pub tree_sha: Option<&'a str>,
    pub author_name: Option<&'a str>,
    pub author_email: Option<&'a str>,
    pub authored_at: Option<&'a str>,
    pub committer_name: Option<&'a str>,
    pub committer_email: Option<&'a str>,
    pub committed_at: Option<&'a str>,
    pub message: &'a str,
}

pub fn upsert_commit(conn: &Connection, row: UpsertCommit<'_>) -> Result<()> {
    conn.execute(
        "insert or ignore into commits (
            repository_id, sha, tree_sha, author_name, author_email, authored_at,
            committer_name, committer_email, committed_at, message
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            row.repository_id,
            row.sha,
            row.tree_sha,
            row.author_name,
            row.author_email,
            row.authored_at,
            row.committer_name,
            row.committer_email,
            row.committed_at,
            row.message
        ],
    )?;
    Ok(())
}

pub fn upsert_commit_parent(
    conn: &Connection,
    repository_id: i64,
    commit_sha: &str,
    parent_sha: &str,
    parent_position: i64,
) -> Result<()> {
    conn.execute(
        "insert or ignore into commit_parents (
            repository_id, commit_sha, parent_sha, parent_position
         ) values (?1, ?2, ?3, ?4)",
        params![repository_id, commit_sha, parent_sha, parent_position],
    )?;
    Ok(())
}

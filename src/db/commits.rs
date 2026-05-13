use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Commit {
    pub repository_id: i64,
    pub sha: String,
    pub tree_sha: Option<String>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub authored_at: Option<String>,
    pub committer_name: Option<String>,
    pub committer_email: Option<String>,
    pub committed_at: Option<String>,
    pub message: String,
}

impl Commit {
    pub fn upsert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "insert or ignore into commits (
                repository_id, sha, tree_sha, author_name, author_email, authored_at,
                committer_name, committer_email, committed_at, message
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                self.repository_id,
                self.sha,
                self.tree_sha,
                self.author_name,
                self.author_email,
                self.authored_at,
                self.committer_name,
                self.committer_email,
                self.committed_at,
                self.message,
            ],
        )?;
        Ok(())
    }
}

pub struct CommitParent {
    pub repository_id: i64,
    pub commit_sha: String,
    pub parent_sha: String,
    pub parent_position: i64,
}

impl CommitParent {
    pub fn upsert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "insert or ignore into commit_parents (
                repository_id, commit_sha, parent_sha, parent_position
             ) values (?1, ?2, ?3, ?4)",
            params![
                self.repository_id,
                self.commit_sha,
                self.parent_sha,
                self.parent_position
            ],
        )?;
        Ok(())
    }
}

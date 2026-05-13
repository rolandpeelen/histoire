use anyhow::Result;
use rusqlite::{Connection, params};

use super::BlameReason;

#[derive(Clone)]
pub struct BlameRequest {
    pub id: i64,
    pub scan_id: i64,
    pub commit_sha: String,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub depth: u32,
    pub reason: BlameReason,
}

impl BlameRequest {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        // Saved post-scan, so the status is always 'complete'.
        conn.execute(
            "insert into blame_requests (
                id, scan_id, commit_sha, path, start_line, end_line, depth, reason, status
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'complete')",
            params![
                self.id,
                self.scan_id,
                self.commit_sha,
                self.path,
                self.start_line,
                self.end_line,
                self.depth as i64,
                self.reason.as_str(),
            ],
        )?;
        Ok(())
    }
}

pub struct BlameSpan {
    pub id: i64,
    pub request_id: i64,
    pub repository_id: i64,
    pub blamed_commit_sha: String,
    pub final_commit_sha: String,
    pub final_path: String,
    pub final_start_line: i64,
    pub origin_path: Option<String>,
    pub origin_start_line: Option<i64>,
    pub line_count: i64,
    pub boundary: bool,
    pub diff_hunk_id: Option<i64>,
}

impl BlameSpan {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "insert into blame_spans (
                id, request_id, repository_id, blamed_commit_sha, final_commit_sha,
                final_path, final_start_line, origin_path, origin_start_line,
                line_count, boundary, diff_hunk_id
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                self.id,
                self.request_id,
                self.repository_id,
                self.blamed_commit_sha,
                self.final_commit_sha,
                self.final_path,
                self.final_start_line,
                self.origin_path,
                self.origin_start_line,
                self.line_count,
                self.boundary as i64,
                self.diff_hunk_id,
            ],
        )?;
        Ok(())
    }
}

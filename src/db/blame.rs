use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::BlameReason;

/// Fields needed to insert (or look up) a blame request.
pub struct InsertBlameRequest<'a> {
    pub scan_id: i64,
    pub commit_sha: &'a str,
    pub path: &'a str,
    pub start_line: i64,
    pub end_line: i64,
    pub depth: u32,
    pub reason: BlameReason,
}

/// A row read back from `blame_requests`.
pub struct BlameRequest {
    pub id: i64,
    pub commit_sha: String,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub depth: u32,
}

/// Fields needed to insert a `blame_spans` row.
pub struct InsertBlameSpan<'a> {
    pub request_id: i64,
    pub repository_id: i64,
    pub blamed_commit_sha: &'a str,
    pub final_commit_sha: &'a str,
    pub final_path: &'a str,
    pub final_start_line: i64,
    pub origin_path: Option<&'a str>,
    pub origin_start_line: Option<i64>,
    pub line_count: i64,
    pub boundary: bool,
    pub diff_hunk_id: Option<i64>,
}

/// Insert (or look up, if it already exists) a blame request. Returns the
/// `blame_requests.id` of the row.
pub fn insert_or_get_blame_request(conn: &Connection, row: InsertBlameRequest<'_>) -> Result<i64> {
    conn.execute(
        "insert or ignore into blame_requests (
            scan_id, commit_sha, path, start_line, end_line, depth, reason
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.scan_id,
            row.commit_sha,
            row.path,
            row.start_line,
            row.end_line,
            row.depth as i64,
            row.reason.as_str()
        ],
    )?;
    let id: i64 = conn.query_row(
        "select id from blame_requests
            where scan_id=?1 and commit_sha=?2 and path=?3 and start_line=?4 and end_line=?5",
        params![
            row.scan_id,
            row.commit_sha,
            row.path,
            row.start_line,
            row.end_line
        ],
        |r| r.get(0),
    )?;
    Ok(id)
}

pub fn pick_next_request(conn: &Connection, scan_id: i64) -> Result<Option<i64>> {
    conn.query_row(
        "select id from blame_requests
            where scan_id=?1 and status='queued'
            order by depth asc, id asc
            limit 1",
        params![scan_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn mark_request_complete(conn: &Connection, request_id: i64) -> Result<()> {
    conn.execute(
        "update blame_requests set status='complete' where id=?1",
        params![request_id],
    )?;
    Ok(())
}

pub fn load_request(conn: &Connection, id: i64) -> Result<BlameRequest> {
    conn.query_row(
        "select id, commit_sha, path, start_line, end_line, depth
            from blame_requests where id=?1",
        params![id],
        |r| {
            Ok(BlameRequest {
                id: r.get(0)?,
                commit_sha: r.get(1)?,
                path: r.get(2)?,
                start_line: r.get(3)?,
                end_line: r.get(4)?,
                depth: r.get::<_, i64>(5)? as u32,
            })
        },
    )
    .map_err(Into::into)
}

pub fn insert_blame_span(conn: &Connection, row: InsertBlameSpan<'_>) -> Result<i64> {
    conn.execute(
        "insert into blame_spans (
            request_id, repository_id, blamed_commit_sha, final_commit_sha,
            final_path, final_start_line, origin_path, origin_start_line,
            line_count, boundary, diff_hunk_id
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            row.request_id,
            row.repository_id,
            row.blamed_commit_sha,
            row.final_commit_sha,
            row.final_path,
            row.final_start_line,
            row.origin_path,
            row.origin_start_line,
            row.line_count,
            row.boundary as i64,
            row.diff_hunk_id
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

use anyhow::Result;
use rusqlite::{Connection, params};

use super::FileEventType;

/// Fields needed to insert a `file_events` row.
pub struct InsertFileEvent<'a> {
    pub repository_id: i64,
    pub commit_sha: &'a str,
    pub parent_sha: Option<&'a str>,
    pub parent_position: Option<i64>,
    pub event_type: FileEventType,
    pub old_path: Option<&'a str>,
    pub new_path: Option<&'a str>,
    pub old_blob_sha: Option<&'a str>,
    pub new_blob_sha: Option<&'a str>,
}

/// Fields needed to insert a `diff_hunks` row.
pub struct InsertDiffHunk<'a> {
    pub repository_id: i64,
    pub file_event_id: Option<i64>,
    pub commit_sha: &'a str,
    pub parent_sha: Option<&'a str>,
    pub parent_position: Option<i64>,
    pub old_path: Option<&'a str>,
    pub new_path: Option<&'a str>,
    pub old_start: Option<i64>,
    pub old_lines: Option<i64>,
    pub new_start: Option<i64>,
    pub new_lines: Option<i64>,
    pub patch_text: &'a str,
}

pub fn insert_file_event(conn: &Connection, row: InsertFileEvent<'_>) -> Result<i64> {
    conn.execute(
        "insert into file_events (
            repository_id, commit_sha, parent_sha, parent_position, event_type,
            old_path, new_path, old_blob_sha, new_blob_sha
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            row.repository_id,
            row.commit_sha,
            row.parent_sha,
            row.parent_position,
            row.event_type.as_str(),
            row.old_path,
            row.new_path,
            row.old_blob_sha,
            row.new_blob_sha
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_diff_hunk(conn: &Connection, row: InsertDiffHunk<'_>) -> Result<i64> {
    conn.execute(
        "insert into diff_hunks (
            repository_id, file_event_id, commit_sha, parent_sha, parent_position,
            old_path, new_path, old_start, old_lines, new_start, new_lines, patch_text
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            row.repository_id,
            row.file_event_id,
            row.commit_sha,
            row.parent_sha,
            row.parent_position,
            row.old_path,
            row.new_path,
            row.old_start,
            row.old_lines,
            row.new_start,
            row.new_lines,
            row.patch_text
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_seed_range(
    conn: &Connection,
    scan_id: i64,
    commit_sha: &str,
    path: &str,
    start_line: i64,
    end_line: i64,
    diff_hunk_id: Option<i64>,
) -> Result<i64> {
    conn.execute(
        "insert into seed_ranges (
            scan_id, commit_sha, path, start_line, end_line, diff_hunk_id
         ) values (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            scan_id,
            commit_sha,
            path,
            start_line,
            end_line,
            diff_hunk_id
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

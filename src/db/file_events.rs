use anyhow::Result;
use rusqlite::{Connection, params};

use super::{FileEventType, ParentPos};

pub struct FileEvent {
    pub id: i64,
    pub repository_id: i64,
    pub commit_sha: String,
    pub parent_sha: String,
    pub parent_position: ParentPos,
    pub event_type: FileEventType,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_blob_sha: Option<String>,
    pub new_blob_sha: Option<String>,
}

impl FileEvent {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        let parent_position: Option<i64> = self.parent_position.into();
        conn.execute(
            "insert into file_events (
                id, repository_id, commit_sha, parent_sha, parent_position, event_type,
                old_path, new_path, old_blob_sha, new_blob_sha
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                self.id,
                self.repository_id,
                self.commit_sha,
                self.parent_sha,
                parent_position,
                self.event_type.as_str(),
                self.old_path,
                self.new_path,
                self.old_blob_sha,
                self.new_blob_sha,
            ],
        )?;
        Ok(())
    }
}

pub struct DiffHunk {
    pub id: i64,
    pub repository_id: i64,
    pub file_event_id: i64,
    pub commit_sha: String,
    pub parent_sha: String,
    pub parent_position: ParentPos,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_start: i64,
    pub old_lines: i64,
    pub new_start: i64,
    pub new_lines: i64,
    pub patch_text: String,
}

impl DiffHunk {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        let parent_position: Option<i64> = self.parent_position.into();
        conn.execute(
            "insert into diff_hunks (
                id, repository_id, file_event_id, commit_sha, parent_sha, parent_position,
                old_path, new_path, old_start, old_lines, new_start, new_lines, patch_text
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                self.id,
                self.repository_id,
                self.file_event_id,
                self.commit_sha,
                self.parent_sha,
                parent_position,
                self.old_path,
                self.new_path,
                self.old_start,
                self.old_lines,
                self.new_start,
                self.new_lines,
                self.patch_text,
            ],
        )?;
        Ok(())
    }
}

pub struct SeedRange {
    pub id: i64,
    pub scan_id: i64,
    pub commit_sha: String,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub diff_hunk_id: Option<i64>,
}

impl SeedRange {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "insert into seed_ranges (
                id, scan_id, commit_sha, path, start_line, end_line, diff_hunk_id
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                self.id,
                self.scan_id,
                self.commit_sha,
                self.path,
                self.start_line,
                self.end_line,
                self.diff_hunk_id,
            ],
        )?;
        Ok(())
    }
}

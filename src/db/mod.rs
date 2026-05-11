use anyhow::{Context, Result};
use rusqlite::Connection;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod blame;
mod commits;
mod file_events;
mod lineage;
mod repositories;
mod scans;
mod types;

pub use blame::{
    BlameRequest, InsertBlameRequest, InsertBlameSpan, insert_blame_span,
    insert_or_get_blame_request, load_request, mark_request_complete, pick_next_request,
};
pub use commits::{UpsertCommit, upsert_commit, upsert_commit_parent};
pub use file_events::{
    InsertDiffHunk, InsertFileEvent, insert_diff_hunk, insert_file_event, insert_seed_range,
};
pub use lineage::{InsertLineageEdge, insert_lineage_edge};
pub use repositories::ensure_repository;
pub use scans::{InsertScan, finalize_scan, insert_scan, summarize};
pub use types::{BlameReason, FileEventType, LineageEdgeType};

pub const SCHEMA_SQL: &str = include_str!("schema.sql");

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut buf: OsString = path.as_os_str().to_owned();
    buf.push(suffix);
    PathBuf::from(buf)
}

/// Open a fresh database at `path`, removing any prior file (and its SQLite
/// `-wal` / `-shm` sidecars) so every scan starts from a clean state.
pub fn open_fresh(path: &Path) -> Result<Connection> {
    for victim in [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ] {
        if victim.exists() {
            std::fs::remove_file(&victim)
                .with_context(|| format!("removing {}", victim.display()))?;
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating db parent dir {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("opening database at {}", path.display()))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;
        "#,
    )?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_appends_suffix_for_extension_path() {
        let p = Path::new("/tmp/foo/db.sqlite");
        assert_eq!(sidecar(p, "-wal"), PathBuf::from("/tmp/foo/db.sqlite-wal"));
        assert_eq!(sidecar(p, "-shm"), PathBuf::from("/tmp/foo/db.sqlite-shm"));
    }

    #[test]
    fn sidecar_appends_suffix_when_no_extension() {
        let p = Path::new("histoire");
        assert_eq!(sidecar(p, "-wal"), PathBuf::from("histoire-wal"));
    }
}

use anyhow::Result;
use rusqlite::{Connection, params};

/// Fields needed to insert a `scans` row.
pub struct InsertScan<'a> {
    pub repository_id: i64,
    pub base_ref: &'a str,
    pub base_sha: &'a str,
    pub merge_base_sha: &'a str,
    pub head_sha: &'a str,
    pub max_depth: u32,
    pub since_date: &'a str,
}

pub struct ScanSummary {
    pub seed_files: i64,
    pub seed_ranges: i64,
    pub requests_processed: i64,
    pub commits_discovered: i64,
    pub terminal_spans: i64,
}

pub fn insert_scan(conn: &Connection, row: InsertScan<'_>) -> Result<i64> {
    conn.execute(
        "insert into scans (
            repository_id, base_ref, base_sha, merge_base_sha, head_sha,
            max_depth, since_date
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.repository_id,
            row.base_ref,
            row.base_sha,
            row.merge_base_sha,
            row.head_sha,
            row.max_depth as i64,
            row.since_date
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finalize_scan(conn: &Connection, scan_id: i64) -> Result<()> {
    conn.execute(
        "update scans set status='complete', finished_at=current_timestamp where id=?1",
        params![scan_id],
    )?;
    Ok(())
}

pub fn summarize(conn: &Connection, scan_id: i64, repository_id: i64) -> Result<ScanSummary> {
    let seed_files: i64 = conn.query_row(
        "select count(distinct path) from seed_ranges where scan_id=?1",
        params![scan_id],
        |r| r.get(0),
    )?;
    let seed_ranges: i64 = conn.query_row(
        "select count(*) from seed_ranges where scan_id=?1",
        params![scan_id],
        |r| r.get(0),
    )?;
    let requests_processed: i64 = conn.query_row(
        "select count(*) from blame_requests where scan_id=?1 and status='complete'",
        params![scan_id],
        |r| r.get(0),
    )?;
    let commits_discovered: i64 = conn.query_row(
        "select count(*) from commits where repository_id=?1",
        params![repository_id],
        |r| r.get(0),
    )?;
    let terminal_spans: i64 = conn.query_row(
        "select count(*) from lineage_edges
            where scan_id=?1 and edge_type<>'recurse_to_parent'",
        params![scan_id],
        |r| r.get(0),
    )?;
    Ok(ScanSummary {
        seed_files,
        seed_ranges,
        requests_processed,
        commits_discovered,
        terminal_spans,
    })
}

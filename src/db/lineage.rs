use anyhow::Result;
use rusqlite::{Connection, params};

use super::LineageEdgeType;

/// Fields needed to insert a `lineage_edges` row.
pub struct InsertLineageEdge<'a> {
    pub scan_id: i64,
    pub from_request_id: i64,
    pub to_span_id: i64,
    pub parent_sha: Option<&'a str>,
    pub parent_position: Option<i64>,
    pub next_request_id: Option<i64>,
    pub edge_type: LineageEdgeType,
}

pub fn insert_lineage_edge(conn: &Connection, row: InsertLineageEdge<'_>) -> Result<i64> {
    conn.execute(
        "insert into lineage_edges (
            scan_id, from_request_id, to_span_id, parent_sha, parent_position,
            next_request_id, edge_type
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.scan_id,
            row.from_request_id,
            row.to_span_id,
            row.parent_sha,
            row.parent_position,
            row.next_request_id,
            row.edge_type.as_str()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

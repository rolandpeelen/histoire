use anyhow::Result;
use rusqlite::{Connection, params};

use super::LineageEdgeType;

pub struct LineageEdge {
    pub id: i64,
    pub scan_id: i64,
    pub from_request_id: i64,
    pub to_span_id: i64,
    pub parent_sha: Option<String>,
    /// The parent index of the blamed commit this edge crosses through.
    /// `None` only for terminal edges that don't involve a parent (e.g. `RootCommit`).
    pub parent_position: Option<i64>,
    pub next_request_id: Option<i64>,
    pub edge_type: LineageEdgeType,
}

impl LineageEdge {
    pub fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "insert into lineage_edges (
                id, scan_id, from_request_id, to_span_id, parent_sha, parent_position,
                next_request_id, edge_type
             ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                self.id,
                self.scan_id,
                self.from_request_id,
                self.to_span_id,
                self.parent_sha,
                self.parent_position,
                self.next_request_id,
                self.edge_type.as_str(),
            ],
        )?;
        Ok(())
    }
}

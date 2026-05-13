use anyhow::Result;
use rusqlite::Connection;

use super::{
    BlameRequest, BlameSpan, Commit, CommitParent, DiffHunk, FileEvent, LineageEdge, Repository,
    ScanRow, SeedRange,
};

/// Everything one scan produces: the per-scan metadata row plus every dependent
/// row, all built in-memory by the scanner. Persistence lives here (not in the
/// scanner) so the `scan` module never needs to know about SQLite.
pub struct Scan {
    pub repository: Repository,
    pub row: ScanRow,
    pub commits: Vec<Commit>,
    pub commit_parents: Vec<CommitParent>,
    pub file_events: Vec<FileEvent>,
    pub diff_hunks: Vec<DiffHunk>,
    pub seed_ranges: Vec<SeedRange>,
    pub blame_requests: Vec<BlameRequest>,
    pub blame_spans: Vec<BlameSpan>,
    pub lineage_edges: Vec<LineageEdge>,
}

impl Scan {
    /// Persist every row in this scan to `conn` in a single transaction.
    pub fn save(&self, conn: &mut Connection) -> Result<()> {
        let transaction = conn.transaction()?;
        self.repository.insert(&transaction)?;
        self.row.insert(&transaction)?;
        self.commits
            .iter()
            .try_for_each(|row| row.upsert(&transaction))?;
        self.commit_parents
            .iter()
            .try_for_each(|row| row.upsert(&transaction))?;
        self.file_events
            .iter()
            .try_for_each(|row| row.insert(&transaction))?;
        self.diff_hunks
            .iter()
            .try_for_each(|row| row.insert(&transaction))?;
        self.seed_ranges
            .iter()
            .try_for_each(|row| row.insert(&transaction))?;
        self.blame_requests
            .iter()
            .try_for_each(|row| row.insert(&transaction))?;
        self.blame_spans
            .iter()
            .try_for_each(|row| row.insert(&transaction))?;
        self.lineage_edges
            .iter()
            .try_for_each(|row| row.insert(&transaction))?;
        transaction.commit()?;
        Ok(())
    }
}

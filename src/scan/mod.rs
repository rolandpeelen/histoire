//! Walk every line that the current branch added back through history.
//!
//! Two phases:
//!
//! 1. [`run`] does a pure, in-memory blame traversal of every added range from
//!    merge-base→HEAD. It returns the full traversal as rows in a [`ScanResult`].
//!    No database writes happen here.
//! 2. The caller writes the [`ScanResult`] to SQLite via [`crate::db::save`].

use anyhow::{Result, anyhow};
use tracing::{info, warn};

use crate::cli::{ScanArgs, default_since};
use crate::db::{
    BlameRequest, BlameSpan, Commit, CommitParent, DiffHunk, FileEvent, LineageEdge,
    LineageEdgeType, Repository, Scan, SeedRange,
};
use crate::git_ops::{DiffFileEvent, head_commit, merge_base, open_repo, resolve_ref};

mod plan;
mod scanner;

use scanner::Scanner;

pub struct ScanResult {
    pub repository: Repository,
    pub scan: Scan,
    pub commits: Vec<Commit>,
    pub commit_parents: Vec<CommitParent>,
    pub file_events: Vec<FileEvent>,
    pub diff_hunks: Vec<DiffHunk>,
    pub seed_ranges: Vec<SeedRange>,
    pub blame_requests: Vec<BlameRequest>,
    pub blame_spans: Vec<BlameSpan>,
    pub lineage_edges: Vec<LineageEdge>,
}

pub struct ScanSummary {
    pub seed_files: i64,
    pub seed_ranges: i64,
    pub requests_processed: i64,
    pub commits_discovered: i64,
    pub terminal_spans: i64,
}

impl ScanResult {
    pub fn summary(&self) -> ScanSummary {
        let seed_files = self
            .seed_ranges
            .iter()
            .map(|range| range.path.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len() as i64;
        let terminal_spans = self
            .lineage_edges
            .iter()
            .filter(|edge| edge.edge_type != LineageEdgeType::RecurseToParent)
            .count() as i64;
        ScanSummary {
            seed_files,
            seed_ranges: self.seed_ranges.len() as i64,
            requests_processed: self.blame_requests.len() as i64,
            commits_discovered: self.commits.len() as i64,
            terminal_spans,
        }
    }
}

/// One diff cached so the planner can look it up by commit + parent-slot.
struct PersistedEvent {
    info: DiffFileEvent,
    /// `diff_hunks.id` per element of `info.hunks`, in the same order.
    hunk_ids: Vec<i64>,
}

struct PersistedDiff {
    events: Vec<PersistedEvent>,
}

/// What to do for one `(span, parent)` pair. `plan::plan_parent_recursion`
/// produces these as pure values; the Scanner applies them.
enum RecurseAction {
    Terminal {
        span_id: i64,
        edge_type: LineageEdgeType,
    },
    Recurse {
        span_id: i64,
        parent_path: String,
        parent_start: i64,
        parent_end: i64,
    },
}

pub fn run(cli: &ScanArgs) -> Result<ScanResult> {
    let repo = open_repo()?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("histoire must be run inside a non-bare working tree"))?
        .to_path_buf();
    let git_dir = repo.path().to_path_buf();
    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(String::from));

    let head = head_commit(&repo)?;
    let head_id = head.id();
    let head_sha = head_id.to_string();
    info!("HEAD: {head_sha}");

    let base_commit = match resolve_ref(&repo, &cli.base_ref) {
        Ok(commit) => Some(commit),
        Err(e) => {
            warn!(
                "base ref '{}' not found ({}); recording empty scan",
                cli.base_ref, e
            );
            None
        }
    };
    let (base_sha, merge_base_id) = match &base_commit {
        Some(commit) => {
            let id = commit.id();
            (id.to_string(), merge_base(&repo, id, head_id)?)
        }
        None => (head_sha.clone(), head_id),
    };
    info!("base: {} merge-base: {}", base_sha, merge_base_id);

    let since = cli.since.unwrap_or_else(default_since);
    info!("max-depth: {} since: {}", cli.max_depth, since);

    let repository = Repository {
        id: 1,
        worktree_path: workdir.to_string_lossy().into_owned(),
        git_dir_path: git_dir.to_string_lossy().into_owned(),
        remote_url,
    };
    let scan = Scan {
        id: 1,
        repository_id: repository.id,
        base_ref: cli.base_ref.clone(),
        base_sha,
        merge_base_sha: merge_base_id.to_string(),
        head_sha,
        max_depth: cli.max_depth,
        since_date: since.to_string(),
    };

    let mut scanner = Scanner::new(
        &repo,
        repository,
        scan,
        since,
        cli.max_depth,
        cli.rename_threshold,
        cli.include_binary,
    );
    scanner.seed(merge_base_id, head_id)?;
    scanner.drain_queue()?;
    Ok(scanner.into_result())
}

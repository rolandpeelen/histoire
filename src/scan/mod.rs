//! Walk every line that the current branch added back through history.
//!
//! Two phases:
//!
//! 1. [`run_scan`] / [`run_trace`] do a pure, in-memory blame traversal — branch
//!    mode seeds from every added range in `merge_base..HEAD`, trace mode seeds
//!    from a single `path:line` target. Both return a [`db::Scan`]. No database
//!    writes happen here.
//! 2. The caller writes the [`db::Scan`] to SQLite via [`db::Scan::save`].

use anyhow::{Result, anyhow};
use git2::{Oid, Repository as GitRepository};
use tracing::{info, warn};

use crate::cli::{ScanArgs, TraceArgs, TraceTarget, default_scan_since, default_trace_since};
use crate::db::{self, LineageEdgeType, Repository, ScanRow};
use crate::git_ops::{DiffFileEvent, head_commit, merge_base, open_repo, resolve_ref};

mod plan;
mod scanner;

use scanner::Scanner;
pub use scanner::SeedSpec;

pub struct ScanSummary {
    pub seed_files: i64,
    pub seed_ranges: i64,
    pub requests_processed: i64,
    pub commits_discovered: i64,
    pub terminal_spans: i64,
}

pub fn summary(scan: &db::Scan) -> ScanSummary {
    let seed_files = scan
        .seed_ranges
        .iter()
        .map(|range| range.path.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len() as i64;
    let terminal_spans = scan
        .lineage_edges
        .iter()
        .filter(|edge| edge.edge_type != LineageEdgeType::RecurseToParent)
        .count() as i64;
    ScanSummary {
        seed_files,
        seed_ranges: scan.seed_ranges.len() as i64,
        requests_processed: scan.blame_requests.len() as i64,
        commits_discovered: scan.commits.len() as i64,
        terminal_spans,
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

struct RepoHead {
    repository: Repository,
    head_id: Oid,
    head_sha: String,
}

fn repo_head(repo: &GitRepository) -> Result<RepoHead> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("histoire must be run inside a non-bare working tree"))?
        .to_path_buf();
    let git_dir = repo.path().to_path_buf();
    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|remote| remote.url().map(String::from));

    let head = head_commit(repo)?;
    let head_id = head.id();
    let head_sha = head_id.to_string();
    info!("HEAD: {head_sha}");

    Ok(RepoHead {
        repository: Repository {
            id: 1,
            worktree_path: workdir.to_string_lossy().into_owned(),
            git_dir_path: git_dir.to_string_lossy().into_owned(),
            remote_url,
        },
        head_id,
        head_sha,
    })
}

/// Recursively trace history backward from every line added on the current branch.
pub fn run_scan(cli: &ScanArgs) -> Result<db::Scan> {
    let repo = open_repo()?;
    let context = repo_head(&repo)?;

    let base_commit = match resolve_ref(&repo, &cli.base_ref) {
        Ok(commit) => Some(commit),
        Err(error) => {
            warn!(
                "base ref '{}' not found ({}); recording empty scan",
                cli.base_ref, error
            );
            None
        }
    };
    let (base_sha, merge_base_id) = match &base_commit {
        Some(commit) => {
            let id = commit.id();
            (id.to_string(), merge_base(&repo, id, context.head_id)?)
        }
        None => (context.head_sha.clone(), context.head_id),
    };
    info!("base: {} merge-base: {}", base_sha, merge_base_id);

    let since = cli.since.unwrap_or_else(default_scan_since);
    info!("max-depth: {} since: {}", cli.max_depth, since);

    let row = ScanRow {
        id: 1,
        repository_id: context.repository.id,
        base_ref: cli.base_ref.clone(),
        base_sha,
        merge_base_sha: merge_base_id.to_string(),
        head_sha: context.head_sha,
        max_depth: cli.max_depth,
        since_date: since.to_string(),
    };

    Scanner::new(
        &repo,
        context.repository,
        row,
        since,
        cli.max_depth,
        cli.rename_threshold,
        cli.include_binary,
        SeedSpec::Branch {
            merge_base_id,
            head_id: context.head_id,
        },
    )?
    .run()
}

/// Recursively trace history backward from a single `path:line` target at HEAD.
pub fn run_trace(cli: &TraceArgs) -> Result<db::Scan> {
    let target = TraceTarget::parse(&cli.target)?;
    let repo = open_repo()?;
    let context = repo_head(&repo)?;

    info!(
        "trace target: {}:{}-{}",
        target.path, target.start_line, target.end_line
    );

    let since = cli.since.unwrap_or_else(default_trace_since);
    info!("max-depth: {} since: {}", cli.max_depth, since);

    let base_ref = format!(
        "trace:{}:{}-{}",
        target.path, target.start_line, target.end_line
    );
    let row = ScanRow {
        id: 1,
        repository_id: context.repository.id,
        base_ref,
        base_sha: context.head_sha.clone(),
        merge_base_sha: context.head_sha.clone(),
        head_sha: context.head_sha,
        max_depth: cli.max_depth,
        since_date: since.to_string(),
    };

    Scanner::new(
        &repo,
        context.repository,
        row,
        since,
        cli.max_depth,
        cli.rename_threshold,
        cli.include_binary,
        SeedSpec::Line {
            head_id: context.head_id,
            path: target.path,
            start_line: target.start_line,
            end_line: target.end_line,
        },
    )?
    .run()
}

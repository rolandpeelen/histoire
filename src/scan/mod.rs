use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::cli::{ScanArgs, default_since};
use crate::db::{self, InsertScan};
use crate::git_ops::{DiffFileEvent, head_commit, merge_base, open_repo, resolve_ref};

mod plan;
mod scanner;

use scanner::Scanner;

/// Which "parent slot" a cached diff corresponds to. `Seed` is the
/// merge-base→HEAD diff that initiates the scan; `Index(n)` is the n-th
/// parent of a commit visited during recursion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ParentPos {
    Seed,
    Index(u32),
}

impl ParentPos {
    fn to_sql(self) -> Option<i64> {
        match self {
            Self::Seed => None,
            Self::Index(i) => Some(i64::from(i)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DiffKey {
    commit_sha: String,
    position: ParentPos,
}

struct PersistedEvent {
    info: DiffFileEvent,
    /// `diff_hunks.id` per element of `info.hunks`, in the same order.
    hunk_ids: Vec<i64>,
}

/// A diff after its `file_events` and `diff_hunks` rows have been written.
/// We keep the hunk IDs so the seed insert can reference them.
struct PersistedDiff {
    events: Vec<PersistedEvent>,
}

/// What to do for one `(span, parent)` pair. `plan_parent_recursion`
/// produces these without touching the database; `Scanner::write_parent_actions`
/// applies them.
enum RecurseAction {
    Terminal {
        span_id: i64,
        edge_type: crate::db::LineageEdgeType,
    },
    Recurse {
        span_id: i64,
        parent_path: String,
        parent_start: i64,
        parent_end: i64,
    },
}

fn resolve_db_path(cli: &ScanArgs, git_dir: &Path) -> PathBuf {
    cli.db
        .clone()
        .unwrap_or_else(|| git_dir.join("histoire.sqlite"))
}

pub fn run(cli: &ScanArgs) -> Result<()> {
    let repo = open_repo()?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("histoire must be run inside a non-bare working tree"))?
        .to_path_buf();
    let git_dir = repo.path().to_path_buf();
    let db_path = resolve_db_path(cli, &git_dir);
    info!("database: {}", db_path.display());

    let mut conn = db::open_fresh(&db_path)?;
    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(String::from));
    let repository_id = db::ensure_repository(
        &conn,
        &workdir.to_string_lossy(),
        &git_dir.to_string_lossy(),
        remote_url.as_deref(),
    )?;

    let head = head_commit(&repo)?;
    let head_oid = head.id();
    let head_sha = head_oid.to_string();
    info!("HEAD: {head_sha}");

    let base_commit = match resolve_ref(&repo, &cli.base_ref) {
        Ok(c) => Some(c),
        Err(e) => {
            warn!(
                "base ref '{}' not found ({}); recording empty scan",
                cli.base_ref, e
            );
            None
        }
    };

    let (base_sha, merge_base_oid) = match &base_commit {
        Some(b) => {
            let oid = b.id();
            (oid.to_string(), merge_base(&repo, oid, head_oid)?)
        }
        None => (head_sha.clone(), head_oid),
    };
    info!("base: {} merge-base: {}", base_sha, merge_base_oid);

    let since = cli.since.unwrap_or_else(default_since);
    info!("max-depth: {} since: {}", cli.max_depth, since);

    let merge_base_sha = merge_base_oid.to_string();
    let since_date = since.to_string();
    let scan_id = db::insert_scan(
        &conn,
        InsertScan {
            repository_id,
            base_ref: &cli.base_ref,
            base_sha: &base_sha,
            merge_base_sha: &merge_base_sha,
            head_sha: &head_sha,
            max_depth: cli.max_depth,
            since_date: &since_date,
        },
    )?;

    let mut scanner = Scanner {
        conn: &mut conn,
        repo: &repo,
        repository_id,
        scan_id,
        max_depth: cli.max_depth,
        since,
        rename_threshold: cli.rename_threshold,
        include_binary: cli.include_binary,
        commit_cache: HashMap::new(),
        diff_cache: HashMap::new(),
    };
    scanner.seed(merge_base_oid, head_oid)?;
    scanner.drain_queue()?;
    db::finalize_scan(scanner.conn, scan_id)?;

    let summary = db::summarize(&conn, scan_id, repository_id)?;
    info!(
        "scan complete: seed_files={}, seed_ranges={}, requests_processed={}, commits_discovered={}, terminal_spans={}",
        summary.seed_files,
        summary.seed_ranges,
        summary.requests_processed,
        summary.commits_discovered,
        summary.terminal_spans
    );
    Ok(())
}

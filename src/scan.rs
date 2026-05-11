use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use git2::{Oid, Repository};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::cli::{ScanArgs, default_since};
use crate::db::{
    self, BlameReason, FileEventType, InsertBlameRequest, InsertBlameSpan, InsertDiffHunk,
    InsertFileEvent, InsertLineageEdge, InsertScan, LineageEdgeType, UpsertCommit,
};
use crate::git_ops::{
    BlameSpan, CommitInfo, DiffFileEvent, collect_diff_events, commit_info, compute_diff,
    head_commit, merge_base, open_repo, resolve_ref, run_blame,
};

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

fn resolve_db_path(cli: &ScanArgs, git_dir: &Path) -> PathBuf {
    cli.db
        .clone()
        .unwrap_or_else(|| git_dir.join("histoire.sqlite"))
}

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

/// A diff after its `file_events` and `diff_hunks` rows have been written.
/// We keep the hunk IDs so the seed insert can reference them.
struct PersistedDiff {
    events: Vec<PersistedEvent>,
}

struct PersistedEvent {
    info: DiffFileEvent,
    /// `diff_hunks.id` per element of `info.hunks`, in the same order.
    hunk_ids: Vec<i64>,
}

/// What to do for one `(span, parent)` pair. `plan_parent_recursion`
/// produces these without touching the database; `write_parent_actions`
/// applies them.
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

struct Scanner<'a> {
    conn: &'a mut Connection,
    repo: &'a Repository,
    repository_id: i64,
    scan_id: i64,
    max_depth: u32,
    since: NaiveDate,
    rename_threshold: u16,
    include_binary: bool,
    commit_cache: HashMap<String, CommitInfo>,
    diff_cache: HashMap<DiffKey, PersistedDiff>,
}

impl Scanner<'_> {
    fn seed(&mut self, merge_base_oid: Oid, head_oid: Oid) -> Result<()> {
        if merge_base_oid == head_oid {
            info!("merge-base equals HEAD; nothing to seed");
            return Ok(());
        }
        self.store_commit(head_oid)?;
        let persisted = self.populate_diff(head_oid, merge_base_oid, ParentPos::Seed)?;
        let head_sha = head_oid.to_string();

        let tx = self.conn.transaction()?;
        for event in &persisted.events {
            let Some(new_path) = event.info.new_path.as_deref() else {
                continue;
            };
            for (hunk, &hunk_id) in event.info.hunks.iter().zip(&event.hunk_ids) {
                for &(start, end) in &hunk.added_ranges {
                    db::insert_seed_range(
                        &tx,
                        self.scan_id,
                        &head_sha,
                        new_path,
                        i64::from(start),
                        i64::from(end),
                        Some(hunk_id),
                    )?;
                    db::insert_or_get_blame_request(
                        &tx,
                        InsertBlameRequest {
                            scan_id: self.scan_id,
                            commit_sha: &head_sha,
                            path: new_path,
                            start_line: i64::from(start),
                            end_line: i64::from(end),
                            depth: 0,
                            reason: BlameReason::Seed,
                        },
                    )?;
                }
            }
        }
        tx.commit()?;

        self.diff_cache.insert(
            DiffKey {
                commit_sha: head_sha,
                position: ParentPos::Seed,
            },
            persisted,
        );
        Ok(())
    }

    fn drain_queue(&mut self) -> Result<()> {
        while let Some(req_id) = db::pick_next_request(self.conn, self.scan_id)? {
            self.process_request(req_id)?;
            db::mark_request_complete(self.conn, req_id)?;
        }
        Ok(())
    }

    fn process_request(&mut self, request_id: i64) -> Result<()> {
        let req = db::load_request(self.conn, request_id)?;
        debug!(
            "process request #{} commit={} path={} {}..{} depth={}",
            req.id, req.commit_sha, req.path, req.start_line, req.end_line, req.depth
        );

        let commit_oid = Oid::from_str(&req.commit_sha)
            .with_context(|| format!("parsing commit OID {}", req.commit_sha))?;
        let spans = run_blame(
            self.repo,
            commit_oid,
            &req.path,
            req.start_line as u32,
            req.end_line as u32,
        )?;

        let spans_by_commit = self.persist_blame_spans(&req, spans)?;
        for blamed_sha in spans_by_commit.keys() {
            let oid = Oid::from_str(blamed_sha)
                .with_context(|| format!("parsing blamed OID {blamed_sha}"))?;
            self.store_commit(oid)?;
        }
        for (blamed_sha, spans) in spans_by_commit {
            // Snapshot the commit metadata so we can release the cache borrow
            // before re-borrowing `self` mutably to write edges/requests.
            let info_snapshot = self
                .commit_cache
                .get(&blamed_sha)
                .ok_or_else(|| anyhow!("commit info missing for {blamed_sha}"))?
                .clone();
            self.handle_blamed_commit(&req, &info_snapshot, spans)?;
        }
        Ok(())
    }

    /// Insert every blame span from one request, returning them grouped by
    /// blamed commit so the recursion step can fan out per ancestor.
    fn persist_blame_spans(
        &mut self,
        req: &db::BlameRequest,
        spans: Vec<BlameSpan>,
    ) -> Result<HashMap<String, Vec<(i64, BlameSpan)>>> {
        let mut by_commit: HashMap<String, Vec<(i64, BlameSpan)>> = HashMap::new();
        let tx = self.conn.transaction()?;
        for span in spans {
            let span_id = db::insert_blame_span(
                &tx,
                InsertBlameSpan {
                    request_id: req.id,
                    repository_id: self.repository_id,
                    blamed_commit_sha: &span.blamed_commit_sha,
                    final_commit_sha: &req.commit_sha,
                    final_path: &req.path,
                    final_start_line: i64::from(span.final_start_line),
                    origin_path: span.origin_path.as_deref(),
                    origin_start_line: span.origin_start_line.map(i64::from),
                    line_count: i64::from(span.line_count),
                    boundary: span.boundary,
                    diff_hunk_id: None,
                },
            )?;
            by_commit
                .entry(span.blamed_commit_sha.clone())
                .or_default()
                .push((span_id, span));
        }
        tx.commit()?;
        Ok(by_commit)
    }

    fn handle_blamed_commit(
        &mut self,
        req: &db::BlameRequest,
        info: &CommitInfo,
        spans: Vec<(i64, BlameSpan)>,
    ) -> Result<()> {
        match classify_terminal(req, info, self.max_depth, self.since) {
            Some(edge_type) => self.write_terminal_for_all(req, &spans, edge_type),
            None => self.recurse_through_parents(req, info, &spans),
        }
    }

    fn write_terminal_for_all(
        &mut self,
        req: &db::BlameRequest,
        spans: &[(i64, BlameSpan)],
        edge_type: LineageEdgeType,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (span_id, _) in spans {
            db::insert_lineage_edge(
                &tx,
                InsertLineageEdge {
                    scan_id: self.scan_id,
                    from_request_id: req.id,
                    to_span_id: *span_id,
                    parent_sha: None,
                    parent_position: None,
                    next_request_id: None,
                    edge_type,
                },
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn recurse_through_parents(
        &mut self,
        req: &db::BlameRequest,
        info: &CommitInfo,
        spans: &[(i64, BlameSpan)],
    ) -> Result<()> {
        let blamed_oid = Oid::from_str(&info.sha)?;
        for (idx, parent_sha) in info.parents.iter().enumerate() {
            let position = ParentPos::Index(idx as u32);
            let parent_oid = Oid::from_str(parent_sha)?;
            self.ensure_diff_cached(blamed_oid, parent_oid, position)?;

            let actions = {
                let key = DiffKey {
                    commit_sha: info.sha.clone(),
                    position,
                };
                let cached = self
                    .diff_cache
                    .get(&key)
                    // We just populated this key in ensure_diff_cached; if it
                    // is absent, the in-memory cache is corrupted.
                    .expect("populate_diff just inserted this key");
                plan_parent_recursion(spans, cached)
            };
            self.write_parent_actions(req, parent_sha, position, actions)?;
        }
        Ok(())
    }

    fn write_parent_actions(
        &mut self,
        req: &db::BlameRequest,
        parent_sha: &str,
        position: ParentPos,
        actions: Vec<RecurseAction>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for action in actions {
            match action {
                RecurseAction::Terminal { span_id, edge_type } => {
                    db::insert_lineage_edge(
                        &tx,
                        InsertLineageEdge {
                            scan_id: self.scan_id,
                            from_request_id: req.id,
                            to_span_id: span_id,
                            parent_sha: Some(parent_sha),
                            parent_position: position.to_sql(),
                            next_request_id: None,
                            edge_type,
                        },
                    )?;
                }
                RecurseAction::Recurse {
                    span_id,
                    parent_path,
                    parent_start,
                    parent_end,
                } => {
                    let next_id = db::insert_or_get_blame_request(
                        &tx,
                        InsertBlameRequest {
                            scan_id: self.scan_id,
                            commit_sha: parent_sha,
                            path: &parent_path,
                            start_line: parent_start,
                            end_line: parent_end,
                            depth: req.depth + 1,
                            reason: BlameReason::ParentRecurse,
                        },
                    )?;
                    db::insert_lineage_edge(
                        &tx,
                        InsertLineageEdge {
                            scan_id: self.scan_id,
                            from_request_id: req.id,
                            to_span_id: span_id,
                            parent_sha: Some(parent_sha),
                            parent_position: position.to_sql(),
                            next_request_id: Some(next_id),
                            edge_type: LineageEdgeType::RecurseToParent,
                        },
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn ensure_diff_cached(
        &mut self,
        commit_oid: Oid,
        parent_oid: Oid,
        position: ParentPos,
    ) -> Result<()> {
        let key = DiffKey {
            commit_sha: commit_oid.to_string(),
            position,
        };
        if self.diff_cache.contains_key(&key) {
            return Ok(());
        }
        let persisted = self.populate_diff(commit_oid, parent_oid, position)?;
        self.diff_cache.insert(key, persisted);
        Ok(())
    }

    /// Compute diff(parent → commit), persist its `file_events` and
    /// `diff_hunks` rows, and return the parsed events alongside their
    /// inserted hunk IDs.
    fn populate_diff(
        &mut self,
        commit_oid: Oid,
        parent_oid: Oid,
        position: ParentPos,
    ) -> Result<PersistedDiff> {
        let diff = compute_diff(
            self.repo,
            Some(parent_oid),
            commit_oid,
            self.rename_threshold,
        )?;
        let events = collect_diff_events(&diff)?;
        let commit_sha = commit_oid.to_string();
        let parent_sha = parent_oid.to_string();
        let parent_position = position.to_sql();

        let tx = self.conn.transaction()?;
        let mut persisted_events = Vec::with_capacity(events.len());
        for event in events {
            let event_type = effective_event_type(&event, self.include_binary);
            let file_event_id = db::insert_file_event(
                &tx,
                InsertFileEvent {
                    repository_id: self.repository_id,
                    commit_sha: &commit_sha,
                    parent_sha: Some(&parent_sha),
                    parent_position,
                    event_type,
                    old_path: event.old_path.as_deref(),
                    new_path: event.new_path.as_deref(),
                    old_blob_sha: event.old_blob_sha.as_deref(),
                    new_blob_sha: event.new_blob_sha.as_deref(),
                },
            )?;

            let mut hunk_ids = Vec::with_capacity(event.hunks.len());
            if event_type != FileEventType::BinarySkipped {
                for hunk in &event.hunks {
                    let hunk_id = db::insert_diff_hunk(
                        &tx,
                        InsertDiffHunk {
                            repository_id: self.repository_id,
                            file_event_id: Some(file_event_id),
                            commit_sha: &commit_sha,
                            parent_sha: Some(&parent_sha),
                            parent_position,
                            old_path: event.old_path.as_deref(),
                            new_path: event.new_path.as_deref(),
                            old_start: Some(i64::from(hunk.old_start)),
                            old_lines: Some(i64::from(hunk.old_lines)),
                            new_start: Some(i64::from(hunk.new_start)),
                            new_lines: Some(i64::from(hunk.new_lines)),
                            patch_text: &hunk.patch_text,
                        },
                    )?;
                    hunk_ids.push(hunk_id);
                }
            }
            persisted_events.push(PersistedEvent {
                info: event,
                hunk_ids,
            });
        }
        tx.commit()?;
        Ok(PersistedDiff {
            events: persisted_events,
        })
    }

    fn store_commit(&mut self, oid: Oid) -> Result<()> {
        let sha = oid.to_string();
        if self.commit_cache.contains_key(&sha) {
            return Ok(());
        }
        let info = commit_info(self.repo, oid)?;
        let tx = self.conn.transaction()?;
        db::upsert_commit(
            &tx,
            UpsertCommit {
                repository_id: self.repository_id,
                sha: &info.sha,
                tree_sha: Some(&info.tree_sha),
                author_name: info.author_name.as_deref(),
                author_email: info.author_email.as_deref(),
                authored_at: info.authored_at.as_deref(),
                committer_name: info.committer_name.as_deref(),
                committer_email: info.committer_email.as_deref(),
                committed_at: info.committed_at.as_deref(),
                message: &info.message,
            },
        )?;
        for (idx, parent_sha) in info.parents.iter().enumerate() {
            db::upsert_commit_parent(&tx, self.repository_id, &info.sha, parent_sha, idx as i64)?;
        }
        tx.commit()?;
        self.commit_cache.insert(sha, info);
        Ok(())
    }
}

/// Resolve what `file_events.event_type` should be once `--include-binary`
/// is taken into account. Renames/copies stay typed as renames/copies even
/// when binary, so the rename trail survives in the database.
fn effective_event_type(event: &DiffFileEvent, include_binary: bool) -> FileEventType {
    if !event.is_binary || include_binary {
        return event.event_type;
    }
    match event.event_type {
        FileEventType::Renamed | FileEventType::Copied => event.event_type,
        _ => FileEventType::BinarySkipped,
    }
}

/// Pure check: does this blamed commit hit a recursion cutoff? Returns the
/// terminal edge type if so, or `None` if recursion should proceed.
fn classify_terminal(
    req: &db::BlameRequest,
    info: &CommitInfo,
    max_depth: u32,
    since: NaiveDate,
) -> Option<LineageEdgeType> {
    if info.parents.is_empty() {
        return Some(LineageEdgeType::RootCommit);
    }
    if let Some(d) = info.committed_naive
        && d < since
    {
        return Some(LineageEdgeType::OlderThanSince);
    }
    if req.depth >= max_depth {
        return Some(LineageEdgeType::MaxDepth);
    }
    None
}

/// Pure: for one parent's persisted diff, decide what each span should do.
fn plan_parent_recursion(spans: &[(i64, BlameSpan)], cached: &PersistedDiff) -> Vec<RecurseAction> {
    let mut actions = Vec::new();
    for (span_id, span) in spans {
        plan_for_span(*span_id, span, cached, &mut actions);
    }
    actions
}

fn plan_for_span(
    span_id: i64,
    span: &BlameSpan,
    cached: &PersistedDiff,
    actions: &mut Vec<RecurseAction>,
) {
    let (origin_path, origin_start) = match (span.origin_path.as_deref(), span.origin_start_line) {
        (Some(p), Some(s)) => (p, s),
        _ => {
            actions.push(RecurseAction::Terminal {
                span_id,
                edge_type: LineageEdgeType::IntroducedHere,
            });
            return;
        }
    };
    let origin_end_excl = origin_start + span.line_count;

    let Some(event) = cached
        .events
        .iter()
        .find(|e| e.info.new_path.as_deref() == Some(origin_path))
    else {
        // The path isn't part of this parent's diff (e.g. a merge that pulled
        // the path in from the other side). Nothing to emit for this parent.
        return;
    };

    if event.info.is_binary {
        actions.push(RecurseAction::Terminal {
            span_id,
            edge_type: LineageEdgeType::BinarySkipped,
        });
        return;
    }

    let Some(parent_path) = parent_side_path(&event.info) else {
        actions.push(RecurseAction::Terminal {
            span_id,
            edge_type: LineageEdgeType::IntroducedHere,
        });
        return;
    };

    let mut any_overlap = false;
    for hunk in &event.info.hunks {
        let hunk_new_end_excl = hunk.new_start + hunk.new_lines;
        let overlap_start = origin_start.max(hunk.new_start);
        let overlap_end_excl = origin_end_excl.min(hunk_new_end_excl);
        if overlap_end_excl <= overlap_start {
            continue;
        }
        any_overlap = true;
        if hunk.old_lines == 0 {
            actions.push(RecurseAction::Terminal {
                span_id,
                edge_type: LineageEdgeType::IntroducedHere,
            });
            continue;
        }
        actions.push(RecurseAction::Recurse {
            span_id,
            parent_path: parent_path.clone(),
            parent_start: i64::from(hunk.old_start),
            parent_end: i64::from(hunk.old_start + hunk.old_lines - 1),
        });
    }

    if !any_overlap {
        // The blamed lines exist unchanged in the parent at the same line
        // numbers; recurse on `old_path`, which differs from `new_path` on a
        // rename.
        actions.push(RecurseAction::Recurse {
            span_id,
            parent_path,
            parent_start: i64::from(origin_start),
            parent_end: i64::from(origin_end_excl - 1),
        });
    }
}

fn parent_side_path(event: &DiffFileEvent) -> Option<String> {
    if event.event_type == FileEventType::Added {
        return None;
    }
    event.old_path.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_req(depth: u32) -> db::BlameRequest {
        db::BlameRequest {
            id: 0,
            commit_sha: "x".into(),
            path: "p".into(),
            start_line: 1,
            end_line: 1,
            depth,
        }
    }

    fn dummy_info(parents: Vec<String>, committed: Option<NaiveDate>) -> CommitInfo {
        CommitInfo {
            sha: "x".into(),
            tree_sha: "t".into(),
            author_name: None,
            author_email: None,
            authored_at: None,
            committer_name: None,
            committer_email: None,
            committed_at: None,
            committed_naive: committed,
            message: String::new(),
            parents,
        }
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("test date is valid")
    }

    #[test]
    fn classify_terminal_flags_root_commit() {
        let info = dummy_info(vec![], None);
        let out = classify_terminal(&dummy_req(0), &info, 5, ymd(2020, 1, 1));
        assert_eq!(out, Some(LineageEdgeType::RootCommit));
    }

    #[test]
    fn classify_terminal_flags_older_than_since() {
        let info = dummy_info(vec!["p".into()], Some(ymd(2020, 1, 1)));
        let out = classify_terminal(&dummy_req(0), &info, 5, ymd(2024, 1, 1));
        assert_eq!(out, Some(LineageEdgeType::OlderThanSince));
    }

    #[test]
    fn classify_terminal_flags_max_depth() {
        let info = dummy_info(vec!["p".into()], Some(ymd(2030, 1, 1)));
        let out = classify_terminal(&dummy_req(5), &info, 5, ymd(2020, 1, 1));
        assert_eq!(out, Some(LineageEdgeType::MaxDepth));
    }

    #[test]
    fn classify_terminal_allows_recursion() {
        let info = dummy_info(vec!["p".into()], Some(ymd(2030, 1, 1)));
        let out = classify_terminal(&dummy_req(0), &info, 5, ymd(2020, 1, 1));
        assert!(out.is_none());
    }
}

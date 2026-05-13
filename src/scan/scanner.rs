use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use git2::{Oid, Repository};
use rusqlite::Connection;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::db::{
    self, BlameReason, FileEventType, InsertBlameRequest, InsertBlameSpan, InsertDiffHunk,
    InsertFileEvent, InsertLineageEdge, LineageEdgeType, UpsertCommit,
};
use crate::git_ops::{
    BlameHunk, CommitInfo, collect_diff_events, commit_info, compute_diff, run_blame,
};

use super::plan::{classify_terminal, effective_event_type, plan_parent_recursion};
use super::{DiffKey, ParentPos, PersistedDiff, PersistedEvent, RecurseAction};

pub(super) struct Scanner<'a> {
    pub conn: &'a mut Connection,
    pub repo: &'a Repository,
    pub repository_id: i64,
    pub scan_id: i64,
    pub max_depth: u32,
    pub since: NaiveDate,
    pub rename_threshold: u16,
    pub include_binary: bool,
    pub commit_cache: HashMap<String, CommitInfo>,
    pub diff_cache: HashMap<DiffKey, PersistedDiff>,
}

impl Scanner<'_> {
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

    fn write_terminal_for_all(
        &mut self,
        req: &db::BlameRequest,
        spans: &[(i64, BlameHunk)],
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
        spans: &[(i64, BlameHunk)],
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

    fn handle_blamed_commit(
        &mut self,
        req: &db::BlameRequest,
        info: &CommitInfo,
        spans: Vec<(i64, BlameHunk)>,
    ) -> Result<()> {
        match classify_terminal(req, info, self.max_depth, self.since) {
            Some(edge_type) => self.write_terminal_for_all(req, &spans, edge_type),
            None => self.recurse_through_parents(req, info, &spans),
        }
    }

    /// Insert every blame span from one request, returning them grouped by
    /// blamed commit so the recursion step can fan out per ancestor.
    fn persist_blame_spans(
        &mut self,
        req: &db::BlameRequest,
        spans: Vec<BlameHunk>,
    ) -> Result<HashMap<String, Vec<(i64, BlameHunk)>>> {
        let mut by_commit: HashMap<String, Vec<(i64, BlameHunk)>> = HashMap::new();
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

    pub(super) fn seed(&mut self, merge_base_oid: Oid, head_oid: Oid) -> Result<()> {
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

    pub(super) fn drain_queue(&mut self) -> Result<()> {
        while let Some(req_id) = db::pick_next_request(self.conn, self.scan_id)? {
            self.process_request(req_id)?;
            db::mark_request_complete(self.conn, req_id)?;
        }
        Ok(())
    }
}

use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use git2::{Oid, Repository};
use std::collections::{BTreeMap, VecDeque};
use tracing::{debug, info};

use crate::db::{
    BlameReason, BlameRequest, BlameSpan, Commit, CommitParent, DiffHunk, FileEvent, FileEventType,
    LineageEdge, LineageEdgeType, ParentPos, Repository as DbRepository, Scan as DbScan, ScanRow,
    SeedRange,
};
use crate::git_ops::{
    BlameHunk, CommitInfo, collect_diff_events, commit_info, compute_diff, run_blame,
};

use super::plan::{classify_terminal, effective_event_type, plan_parent_recursion};
use super::{PersistedDiff, PersistedEvent, RecurseAction};

/// How the work queue gets primed. Branch mode seeds from every added line in
/// the `merge_base..HEAD` diff; line mode seeds from a single `path:line(-end)`
/// target at HEAD.
pub enum SeedSpec {
    Branch {
        merge_base_id: Oid,
        head_id: Oid,
    },
    Line {
        head_id: Oid,
        path: String,
        start_line: u32,
        end_line: u32,
    },
}

/// Diff cache key — the (commit, which-parent-slot) pair we already computed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DiffKey {
    commit_sha: String,
    position: ParentPos,
}

/// Dedup key for `blame_requests`. Matches the SQL UNIQUE constraint.
type RequestKey = (String, String, i64, i64);

/// Per-table id allocator, starting at 1 to match SQLite rowid semantics.
struct IdCounter(i64);

impl IdCounter {
    fn new() -> Self {
        Self(1)
    }

    fn alloc(&mut self) -> i64 {
        let id = self.0;
        self.0 += 1;
        id
    }
}

struct NextIds {
    file_event: IdCounter,
    diff_hunk: IdCounter,
    seed_range: IdCounter,
    blame_request: IdCounter,
    blame_span: IdCounter,
    lineage_edge: IdCounter,
}

impl NextIds {
    fn new() -> Self {
        Self {
            file_event: IdCounter::new(),
            diff_hunk: IdCounter::new(),
            seed_range: IdCounter::new(),
            blame_request: IdCounter::new(),
            blame_span: IdCounter::new(),
            lineage_edge: IdCounter::new(),
        }
    }
}

pub struct Scanner<'a> {
    repo: &'a Repository,
    max_depth: u32,
    since: NaiveDate,
    rename_threshold: u16,
    include_binary: bool,
    commit_cache: BTreeMap<String, CommitInfo>,
    diff_cache: BTreeMap<DiffKey, PersistedDiff>,
    request_dedup: BTreeMap<RequestKey, i64>,
    queue: VecDeque<BlameRequest>,
    next_id: NextIds,
    scan: DbScan,
}

impl<'a> Scanner<'a> {
    /// Build a Scanner and prime its work queue from `seed`. Returns a Scanner
    /// ready to be drained via [`Scanner::run`].
    #[allow(clippy::too_many_arguments)] // mirrors the *Args struct knobs
    pub fn new(
        repo: &'a Repository,
        repository: DbRepository,
        row: ScanRow,
        since: NaiveDate,
        max_depth: u32,
        rename_threshold: u16,
        include_binary: bool,
        seed: SeedSpec,
    ) -> Result<Self> {
        let mut scanner = Self {
            repo,
            max_depth,
            since,
            rename_threshold,
            include_binary,
            commit_cache: BTreeMap::new(),
            diff_cache: BTreeMap::new(),
            request_dedup: BTreeMap::new(),
            queue: VecDeque::new(),
            next_id: NextIds::new(),
            scan: DbScan {
                repository,
                row,
                commits: Vec::new(),
                commit_parents: Vec::new(),
                file_events: Vec::new(),
                diff_hunks: Vec::new(),
                seed_ranges: Vec::new(),
                blame_requests: Vec::new(),
                blame_spans: Vec::new(),
                lineage_edges: Vec::new(),
            },
        };
        match seed {
            SeedSpec::Branch {
                merge_base_id,
                head_id,
            } => scanner.seed_branch(merge_base_id, head_id)?,
            SeedSpec::Line {
                head_id,
                path,
                start_line,
                end_line,
            } => scanner.seed_line(head_id, path, start_line, end_line)?,
        }
        Ok(scanner)
    }

    /// Append a `commits` row (and its `commit_parents` rows) for `commit_id`,
    /// unless we've already cached it from a previous visit.
    fn store_commit(&mut self, commit_id: Oid) -> Result<()> {
        let sha = commit_id.to_string();
        if self.commit_cache.contains_key(&sha) {
            return Ok(());
        }
        let info = commit_info(self.repo, commit_id)?;
        let repository_id = self.scan.repository.id;
        self.scan.commits.push(Commit {
            repository_id,
            sha: info.sha.clone(),
            tree_sha: Some(info.tree_sha.clone()),
            author_name: info.author_name.clone(),
            author_email: info.author_email.clone(),
            authored_at: info.authored_at.clone(),
            committer_name: info.committer_name.clone(),
            committer_email: info.committer_email.clone(),
            committed_at: info.committed_at.clone(),
            message: info.message.clone(),
        });
        for (index, parent_sha) in info.parents.iter().enumerate() {
            self.scan.commit_parents.push(CommitParent {
                repository_id,
                commit_sha: info.sha.clone(),
                parent_sha: parent_sha.clone(),
                parent_position: index as i64,
            });
        }
        self.commit_cache.insert(sha, info);
        Ok(())
    }

    /// Compute diff(parent → commit), append its `file_events` and `diff_hunks`
    /// rows, and return the parsed events alongside their allocated hunk IDs.
    fn populate_diff(
        &mut self,
        commit_id: Oid,
        parent_id: Oid,
        position: ParentPos,
    ) -> Result<PersistedDiff> {
        let diff = compute_diff(self.repo, Some(parent_id), commit_id, self.rename_threshold)?;
        let events = collect_diff_events(&diff)?;
        let commit_sha = commit_id.to_string();
        let parent_sha = parent_id.to_string();
        let repository_id = self.scan.repository.id;

        let mut persisted_events = Vec::with_capacity(events.len());
        for event in events {
            let event_type = effective_event_type(&event, self.include_binary);
            let file_event_id = self.next_id.file_event.alloc();
            self.scan.file_events.push(FileEvent {
                id: file_event_id,
                repository_id,
                commit_sha: commit_sha.clone(),
                parent_sha: parent_sha.clone(),
                parent_position: position,
                event_type,
                old_path: event.old_path.clone(),
                new_path: event.new_path.clone(),
                old_blob_sha: event.old_blob_sha.clone(),
                new_blob_sha: event.new_blob_sha.clone(),
            });

            let mut hunk_ids = Vec::with_capacity(event.hunks.len());
            if event_type != FileEventType::BinarySkipped {
                for hunk in &event.hunks {
                    let diff_hunk_id = self.next_id.diff_hunk.alloc();
                    self.scan.diff_hunks.push(DiffHunk {
                        id: diff_hunk_id,
                        repository_id,
                        file_event_id,
                        commit_sha: commit_sha.clone(),
                        parent_sha: parent_sha.clone(),
                        parent_position: position,
                        old_path: event.old_path.clone(),
                        new_path: event.new_path.clone(),
                        old_start: i64::from(hunk.old_start),
                        old_lines: i64::from(hunk.old_lines),
                        new_start: i64::from(hunk.new_start),
                        new_lines: i64::from(hunk.new_lines),
                        patch_text: hunk.patch_text.clone(),
                    });
                    hunk_ids.push(diff_hunk_id);
                }
            }
            persisted_events.push(PersistedEvent {
                info: event,
                hunk_ids,
            });
        }
        Ok(PersistedDiff {
            events: persisted_events,
        })
    }

    /// Insert (or look up) a blame request. Returns its id, whether new or
    /// already enqueued. New requests are pushed onto the work queue.
    fn enqueue_request(
        &mut self,
        commit_sha: String,
        path: String,
        start_line: i64,
        end_line: i64,
        depth: u32,
        reason: BlameReason,
    ) -> i64 {
        let key = (commit_sha.clone(), path.clone(), start_line, end_line);
        if let Some(&id) = self.request_dedup.get(&key) {
            return id;
        }
        let id = self.next_id.blame_request.alloc();
        self.request_dedup.insert(key, id);
        let request = BlameRequest {
            id,
            scan_id: self.scan.row.id,
            commit_sha,
            path,
            start_line,
            end_line,
            depth,
            reason,
        };
        self.queue.push_back(request.clone());
        self.scan.blame_requests.push(request);
        id
    }

    fn ensure_diff_cached(
        &mut self,
        commit_id: Oid,
        parent_id: Oid,
        position: ParentPos,
    ) -> Result<()> {
        let key = DiffKey {
            commit_sha: commit_id.to_string(),
            position,
        };
        if self.diff_cache.contains_key(&key) {
            return Ok(());
        }
        let persisted = self.populate_diff(commit_id, parent_id, position)?;
        self.diff_cache.insert(key, persisted);
        Ok(())
    }

    /// Append a `BlameSpan` row per `BlameHunk`, grouping by blamed commit so
    /// the recursion step can fan out per ancestor.
    fn persist_blame_hunks(
        &mut self,
        request: &BlameRequest,
        hunks: Vec<BlameHunk>,
    ) -> BTreeMap<String, Vec<(i64, BlameHunk)>> {
        let mut by_commit: BTreeMap<String, Vec<(i64, BlameHunk)>> = BTreeMap::new();
        let repository_id = self.scan.repository.id;
        for hunk in hunks {
            let span_id = self.next_id.blame_span.alloc();
            self.scan.blame_spans.push(BlameSpan {
                id: span_id,
                request_id: request.id,
                repository_id,
                blamed_commit_sha: hunk.blamed_commit_sha.clone(),
                final_commit_sha: request.commit_sha.clone(),
                final_path: request.path.clone(),
                final_start_line: i64::from(hunk.final_start_line),
                origin_path: hunk.origin_path.clone(),
                origin_start_line: hunk.origin_start_line.map(i64::from),
                line_count: i64::from(hunk.line_count),
                boundary: hunk.boundary,
                diff_hunk_id: None,
            });
            by_commit
                .entry(hunk.blamed_commit_sha.clone())
                .or_default()
                .push((span_id, hunk));
        }
        by_commit
    }

    fn write_terminal_for_all(
        &mut self,
        request: &BlameRequest,
        spans: &[(i64, BlameHunk)],
        edge_type: LineageEdgeType,
    ) {
        let scan_id = self.scan.row.id;
        for (span_id, _) in spans {
            let edge_id = self.next_id.lineage_edge.alloc();
            self.scan.lineage_edges.push(LineageEdge {
                id: edge_id,
                scan_id,
                from_request_id: request.id,
                to_span_id: *span_id,
                parent_sha: None,
                parent_position: None,
                next_request_id: None,
                edge_type,
            });
        }
    }

    fn write_parent_actions(
        &mut self,
        request: &BlameRequest,
        parent_sha: &str,
        position: ParentPos,
        actions: Vec<RecurseAction>,
    ) {
        let parent_position: Option<i64> = position.into();
        let scan_id = self.scan.row.id;
        for action in actions {
            match action {
                RecurseAction::Terminal { span_id, edge_type } => {
                    let edge_id = self.next_id.lineage_edge.alloc();
                    self.scan.lineage_edges.push(LineageEdge {
                        id: edge_id,
                        scan_id,
                        from_request_id: request.id,
                        to_span_id: span_id,
                        parent_sha: Some(parent_sha.to_string()),
                        parent_position,
                        next_request_id: None,
                        edge_type,
                    });
                }
                RecurseAction::Recurse {
                    span_id,
                    parent_path,
                    parent_start,
                    parent_end,
                } => {
                    let next_request_id = self.enqueue_request(
                        parent_sha.to_string(),
                        parent_path,
                        parent_start,
                        parent_end,
                        request.depth + 1,
                        BlameReason::ParentRecurse,
                    );
                    let edge_id = self.next_id.lineage_edge.alloc();
                    self.scan.lineage_edges.push(LineageEdge {
                        id: edge_id,
                        scan_id,
                        from_request_id: request.id,
                        to_span_id: span_id,
                        parent_sha: Some(parent_sha.to_string()),
                        parent_position,
                        next_request_id: Some(next_request_id),
                        edge_type: LineageEdgeType::RecurseToParent,
                    });
                }
            }
        }
    }

    fn recurse_through_parents(
        &mut self,
        request: &BlameRequest,
        info: &CommitInfo,
        spans: &[(i64, BlameHunk)],
    ) -> Result<()> {
        let blamed_id = Oid::from_str(&info.sha)?;
        for (index, parent_sha) in info.parents.iter().enumerate() {
            let position = ParentPos::Index(index as u32);
            let parent_id = Oid::from_str(parent_sha)?;
            self.ensure_diff_cached(blamed_id, parent_id, position)?;

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
            self.write_parent_actions(request, parent_sha, position, actions);
        }
        Ok(())
    }

    fn process_request(&mut self, request: BlameRequest) -> Result<()> {
        debug!(
            "process request #{} commit={} path={} {}..{} depth={}",
            request.id,
            request.commit_sha,
            request.path,
            request.start_line,
            request.end_line,
            request.depth
        );

        let commit_id = Oid::from_str(&request.commit_sha)
            .with_context(|| format!("parsing commit OID {}", request.commit_sha))?;
        let hunks = run_blame(
            self.repo,
            commit_id,
            &request.path,
            request.start_line as u32,
            request.end_line as u32,
        )?;

        let spans_by_commit = self.persist_blame_hunks(&request, hunks);
        spans_by_commit
            .into_iter()
            .try_for_each(|(blamed_sha, spans)| -> Result<()> {
                let blamed_id = Oid::from_str(&blamed_sha)
                    .with_context(|| format!("parsing blamed OID {blamed_sha}"))?;
                self.store_commit(blamed_id)?;
                let info = self
                    .commit_cache
                    .get(&blamed_sha)
                    .cloned()
                    .ok_or_else(|| anyhow!("commit info missing for {blamed_sha}"))?;
                match classify_terminal(&request, &info, self.max_depth, self.since) {
                    Some(edge_type) => self.write_terminal_for_all(&request, &spans, edge_type),
                    None => self.recurse_through_parents(&request, &info, &spans)?,
                }
                Ok(())
            })
    }

    fn seed_line(
        &mut self,
        head_id: Oid,
        path: String,
        start_line: u32,
        end_line: u32,
    ) -> Result<()> {
        self.store_commit(head_id)?;
        let head_sha = head_id.to_string();
        let scan_id = self.scan.row.id;

        let seed_range_id = self.next_id.seed_range.alloc();
        self.scan.seed_ranges.push(SeedRange {
            id: seed_range_id,
            scan_id,
            commit_sha: head_sha.clone(),
            path: path.clone(),
            start_line: i64::from(start_line),
            end_line: i64::from(end_line),
            diff_hunk_id: None,
        });
        self.enqueue_request(
            head_sha,
            path,
            i64::from(start_line),
            i64::from(end_line),
            0,
            BlameReason::Seed,
        );
        Ok(())
    }

    fn seed_branch(&mut self, merge_base_id: Oid, head_id: Oid) -> Result<()> {
        if merge_base_id == head_id {
            info!("merge-base equals HEAD; nothing to seed");
            return Ok(());
        }
        self.store_commit(head_id)?;
        let persisted = self.populate_diff(head_id, merge_base_id, ParentPos::Seed)?;
        let head_sha = head_id.to_string();
        let scan_id = self.scan.row.id;

        for event in &persisted.events {
            let Some(new_path) = event.info.new_path.as_deref() else {
                continue;
            };
            for (hunk, &hunk_id) in event.info.hunks.iter().zip(&event.hunk_ids) {
                for &(start, end) in &hunk.added_ranges {
                    let seed_range_id = self.next_id.seed_range.alloc();
                    self.scan.seed_ranges.push(SeedRange {
                        id: seed_range_id,
                        scan_id,
                        commit_sha: head_sha.clone(),
                        path: new_path.to_string(),
                        start_line: i64::from(start),
                        end_line: i64::from(end),
                        diff_hunk_id: Some(hunk_id),
                    });
                    self.enqueue_request(
                        head_sha.clone(),
                        new_path.to_string(),
                        i64::from(start),
                        i64::from(end),
                        0,
                        BlameReason::Seed,
                    );
                }
            }
        }

        self.diff_cache.insert(
            DiffKey {
                commit_sha: head_sha,
                position: ParentPos::Seed,
            },
            persisted,
        );
        Ok(())
    }

    /// Drain the work queue and return the assembled scan.
    pub fn run(mut self) -> Result<DbScan> {
        while let Some(request) = self.queue.pop_front() {
            self.process_request(request)?;
        }
        Ok(self.scan)
    }
}

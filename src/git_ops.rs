use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use git2::{
    BlameOptions, Commit, Delta, Diff, DiffFindOptions, DiffOptions, Oid, Patch, Repository,
};
use std::path::Path;

use crate::db::FileEventType;

pub fn open_repo() -> Result<Repository> {
    Repository::discover(".").context("not inside a Git repository")
}

pub fn resolve_ref<'repo>(repo: &'repo Repository, refname: &str) -> Result<Commit<'repo>> {
    let obj = repo
        .revparse_single(refname)
        .with_context(|| format!("resolving ref '{refname}'"))?;
    Ok(obj.peel_to_commit()?)
}

pub fn head_commit(repo: &Repository) -> Result<Commit<'_>> {
    let head = repo.head().context("reading HEAD")?;
    Ok(head.peel_to_commit()?)
}

pub fn merge_base(repo: &Repository, a: Oid, b: Oid) -> Result<Oid> {
    repo.merge_base(a, b)
        .with_context(|| format!("computing merge-base({a}, {b})"))
}

/// Compute a tree-to-tree diff with aggressive rename/copy detection.
///
/// `old_oid = None` produces an against-empty-tree diff (every path looks added).
pub fn compute_diff<'repo>(
    repo: &'repo Repository,
    old_oid: Option<Oid>,
    new_oid: Oid,
    rename_threshold: u16,
) -> Result<Diff<'repo>> {
    let new_tree = repo.find_commit(new_oid)?.tree()?;
    let old_tree = match old_oid {
        Some(o) => Some(repo.find_commit(o)?.tree()?),
        None => None,
    };

    let mut diff_opts = DiffOptions::new();
    diff_opts.ignore_submodules(true);
    diff_opts.context_lines(3);

    let mut diff =
        repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut diff_opts))?;

    let mut find_opts = DiffFindOptions::new();
    find_opts.renames(true);
    find_opts.copies(true);
    find_opts.rename_threshold(rename_threshold);
    find_opts.copy_threshold(rename_threshold);
    diff.find_similar(Some(&mut find_opts))?;

    Ok(diff)
}

/// Metadata about a commit, including the parent SHAs in order.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub tree_sha: String,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub authored_at: Option<String>,
    pub committer_name: Option<String>,
    pub committer_email: Option<String>,
    pub committed_at: Option<String>,
    pub committed_naive: Option<NaiveDate>,
    pub message: String,
    pub parents: Vec<String>,
}

pub fn commit_info(repo: &Repository, oid: Oid) -> Result<CommitInfo> {
    let commit = repo.find_commit(oid)?;
    let author = commit.author();
    let committer = commit.committer();
    let authored_at = signature_time(author.when().seconds());
    let committed_at = signature_time(committer.when().seconds());
    let committed_naive = committed_at.as_ref().and_then(|s| {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc).date_naive())
    });
    let parents: Vec<String> = commit.parent_ids().map(|id| id.to_string()).collect();
    Ok(CommitInfo {
        sha: oid.to_string(),
        tree_sha: commit.tree_id().to_string(),
        author_name: author.name().map(|s| s.to_string()),
        author_email: author.email().map(|s| s.to_string()),
        authored_at,
        committer_name: committer.name().map(|s| s.to_string()),
        committer_email: committer.email().map(|s| s.to_string()),
        committed_at,
        committed_naive,
        message: commit.message().unwrap_or("").to_string(),
        parents,
    })
}

fn signature_time(seconds: i64) -> Option<String> {
    Utc.timestamp_opt(seconds, 0)
        .single()
        .map(|t| t.to_rfc3339())
}

/// One parsed delta from a diff: the event itself plus its hunks (if textual).
#[derive(Debug, Clone)]
pub struct DiffFileEvent {
    pub event_type: FileEventType,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_blob_sha: Option<String>,
    pub new_blob_sha: Option<String>,
    pub is_binary: bool,
    pub hunks: Vec<DiffHunkInfo>,
}

#[derive(Debug, Clone)]
pub struct DiffHunkInfo {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub patch_text: String,
    /// Contiguous added-line ranges on the new side, each `(start, end_inclusive)`.
    pub added_ranges: Vec<(u32, u32)>,
}

pub fn collect_diff_events(diff: &Diff<'_>) -> Result<Vec<DiffFileEvent>> {
    let mut events = Vec::new();
    for (idx, delta) in diff.deltas().enumerate() {
        let old_file = delta.old_file();
        let new_file = delta.new_file();

        let old_path = old_file.path().map(|p| p.to_string_lossy().into_owned());
        let new_path = new_file.path().map(|p| p.to_string_lossy().into_owned());
        let old_blob_sha = (!old_file.id().is_zero()).then(|| old_file.id().to_string());
        let new_blob_sha = (!new_file.id().is_zero()).then(|| new_file.id().to_string());

        let is_binary = old_file.is_binary() || new_file.is_binary();
        // Binary files override the delta status: we don't blame them, regardless
        // of whether git also classifies the change as add/modify/rename.
        let event_type = if is_binary {
            FileEventType::BinarySkipped
        } else {
            map_delta_status(delta.status())
        };

        let mut event = DiffFileEvent {
            event_type,
            old_path,
            new_path,
            old_blob_sha,
            new_blob_sha,
            is_binary,
            hunks: Vec::new(),
        };

        if is_binary {
            events.push(event);
            continue;
        }

        match Patch::from_diff(diff, idx) {
            Ok(Some(patch)) => extract_hunks(&patch, &mut event.hunks)?,
            Ok(None) => {}
            Err(e) => return Err(anyhow!("extracting patch for delta {idx}: {e}")),
        }
        events.push(event);
    }
    Ok(events)
}

fn map_delta_status(status: Delta) -> FileEventType {
    match status {
        Delta::Added => FileEventType::Added,
        Delta::Deleted => FileEventType::Deleted,
        Delta::Modified | Delta::Typechange => FileEventType::Modified,
        Delta::Renamed => FileEventType::Renamed,
        Delta::Copied => FileEventType::Copied,
        _ => FileEventType::Modified,
    }
}

fn extract_hunks(patch: &Patch<'_>, out: &mut Vec<DiffHunkInfo>) -> Result<()> {
    for hunk_idx in 0..patch.num_hunks() {
        let (hunk, _line_count) = patch.hunk(hunk_idx)?;
        let header = std::str::from_utf8(hunk.header()).unwrap_or("").to_string();
        let mut text = header;
        let mut added = AddedRangeCollector::default();

        let lines_in_hunk = patch.num_lines_in_hunk(hunk_idx)?;
        for line_idx in 0..lines_in_hunk {
            let line = patch.line_in_hunk(hunk_idx, line_idx)?;
            text.push(line.origin());
            if let Ok(content) = std::str::from_utf8(line.content()) {
                text.push_str(content);
            }
            if line.origin() == '+'
                && let Some(ln) = line.new_lineno()
            {
                added.push(ln);
            }
        }

        out.push(DiffHunkInfo {
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            patch_text: text,
            added_ranges: added.finish(),
        });
    }
    Ok(())
}

/// Builds contiguous `(start, end_inclusive)` ranges from a stream of added
/// line numbers seen in source order.
#[derive(Default)]
struct AddedRangeCollector {
    ranges: Vec<(u32, u32)>,
    current: Option<(u32, u32)>,
}

impl AddedRangeCollector {
    fn push(&mut self, line: u32) {
        match self.current {
            Some((start, end)) if line == end + 1 => self.current = Some((start, line)),
            Some(range) => {
                self.ranges.push(range);
                self.current = Some((line, line));
            }
            None => self.current = Some((line, line)),
        }
    }

    fn finish(mut self) -> Vec<(u32, u32)> {
        if let Some(range) = self.current.take() {
            self.ranges.push(range);
        }
        self.ranges
    }
}

/// One row from a clipped blame run: a contiguous span on the requested side
/// attributed to a single ancestor commit.
#[derive(Debug, Clone)]
pub struct BlameSpan {
    pub blamed_commit_sha: String,
    pub final_start_line: u32,
    pub line_count: u32,
    pub origin_path: Option<String>,
    pub origin_start_line: Option<u32>,
    pub boundary: bool,
}

pub fn run_blame(
    repo: &Repository,
    commit_oid: Oid,
    path: &str,
    start_line: u32,
    end_line: u32,
) -> Result<Vec<BlameSpan>> {
    let mut opts = BlameOptions::new();
    opts.newest_commit(commit_oid);
    opts.track_copies_same_file(true);
    opts.min_line(start_line as usize);
    opts.max_line(end_line as usize);

    let blame = repo
        .blame_file(Path::new(path), Some(&mut opts))
        .with_context(|| format!("blame {commit_oid}:{path} {start_line}..{end_line}"))?;

    let mut spans = Vec::new();
    for hunk in blame.iter() {
        let final_start = hunk.final_start_line() as u32;
        let lines = hunk.lines_in_hunk() as u32;
        if lines == 0 {
            continue;
        }
        let final_end_excl = final_start + lines;

        let clip_start = final_start.max(start_line);
        let clip_end_excl = final_end_excl.min(end_line + 1);
        if clip_end_excl <= clip_start {
            continue;
        }
        let clip_lines = clip_end_excl - clip_start;
        let offset = clip_start - final_start;

        let orig_start = hunk.orig_start_line() as u32;
        spans.push(BlameSpan {
            blamed_commit_sha: hunk.final_commit_id().to_string(),
            final_start_line: clip_start,
            line_count: clip_lines,
            origin_path: hunk.path().map(|p| p.to_string_lossy().into_owned()),
            origin_start_line: Some(orig_start + offset),
            boundary: hunk.is_boundary(),
        });
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_range_collector_groups_contiguous() {
        let mut c = AddedRangeCollector::default();
        for ln in [10, 11, 12, 20, 21, 30] {
            c.push(ln);
        }
        assert_eq!(c.finish(), vec![(10, 12), (20, 21), (30, 30)]);
    }

    #[test]
    fn added_range_collector_handles_empty() {
        let c = AddedRangeCollector::default();
        assert_eq!(c.finish(), Vec::<(u32, u32)>::new());
    }
}

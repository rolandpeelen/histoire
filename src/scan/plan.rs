use chrono::NaiveDate;

use crate::db::{BlameRequest, FileEventType, LineageEdgeType};
use crate::git_ops::{BlameHunk, CommitInfo, DiffFileEvent};

use super::{PersistedDiff, RecurseAction};

fn parent_side_path(event: &DiffFileEvent) -> Option<String> {
    if event.event_type == FileEventType::Added {
        return None;
    }
    event.old_path.clone()
}

fn plan_for_span(
    span_id: i64,
    span: &BlameHunk,
    cached: &PersistedDiff,
    actions: &mut Vec<RecurseAction>,
) {
    let (Some(origin_path), Some(origin_start)) =
        (span.origin_path.as_deref(), span.origin_start_line)
    else {
        actions.push(RecurseAction::Terminal {
            span_id,
            edge_type: LineageEdgeType::IntroducedHere,
        });
        return;
    };
    let origin_end_excl = origin_start + span.line_count;

    let Some(event) = cached
        .events
        .iter()
        .find(|event| event.info.new_path.as_deref() == Some(origin_path))
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

/// Pure: for one parent's persisted diff, decide what each span should do.
pub(super) fn plan_parent_recursion(
    spans: &[(i64, BlameHunk)],
    cached: &PersistedDiff,
) -> Vec<RecurseAction> {
    let mut actions = Vec::new();
    for (span_id, span) in spans {
        plan_for_span(*span_id, span, cached, &mut actions);
    }
    actions
}

/// Pure check: does this blamed commit hit a recursion cutoff? Returns the
/// terminal edge type if so, or `None` if recursion should proceed.
pub(super) fn classify_terminal(
    request: &BlameRequest,
    info: &CommitInfo,
    max_depth: u32,
    since: NaiveDate,
) -> Option<LineageEdgeType> {
    if info.parents.is_empty() {
        return Some(LineageEdgeType::RootCommit);
    }
    if let Some(committed) = info.committed_naive
        && committed < since
    {
        return Some(LineageEdgeType::OlderThanSince);
    }
    if request.depth >= max_depth {
        return Some(LineageEdgeType::MaxDepth);
    }
    None
}

/// Resolve what `file_events.event_type` should be once `--include-binary`
/// is taken into account. Renames/copies stay typed as renames/copies even
/// when binary, so the rename trail survives in the database.
pub(super) fn effective_event_type(event: &DiffFileEvent, include_binary: bool) -> FileEventType {
    if !event.is_binary || include_binary {
        return event.event_type;
    }
    match event.event_type {
        FileEventType::Renamed | FileEventType::Copied => event.event_type,
        _ => FileEventType::BinarySkipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_request(depth: u32) -> BlameRequest {
        BlameRequest {
            id: 0,
            scan_id: 1,
            commit_sha: "x".into(),
            path: "p".into(),
            start_line: 1,
            end_line: 1,
            depth,
            reason: crate::db::BlameReason::Seed,
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

    fn ymd(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("test date is valid")
    }

    #[test]
    fn classify_terminal_flags_root_commit() {
        let info = dummy_info(vec![], None);
        let out = classify_terminal(&dummy_request(0), &info, 5, ymd(2020, 1, 1));
        assert_eq!(out, Some(LineageEdgeType::RootCommit));
    }

    #[test]
    fn classify_terminal_flags_older_than_since() {
        let info = dummy_info(vec!["p".into()], Some(ymd(2020, 1, 1)));
        let out = classify_terminal(&dummy_request(0), &info, 5, ymd(2024, 1, 1));
        assert_eq!(out, Some(LineageEdgeType::OlderThanSince));
    }

    #[test]
    fn classify_terminal_flags_max_depth() {
        let info = dummy_info(vec!["p".into()], Some(ymd(2030, 1, 1)));
        let out = classify_terminal(&dummy_request(5), &info, 5, ymd(2020, 1, 1));
        assert_eq!(out, Some(LineageEdgeType::MaxDepth));
    }

    #[test]
    fn classify_terminal_allows_recursion() {
        let info = dummy_info(vec!["p".into()], Some(ymd(2030, 1, 1)));
        let out = classify_terminal(&dummy_request(0), &info, 5, ymd(2020, 1, 1));
        assert!(out.is_none());
    }
}

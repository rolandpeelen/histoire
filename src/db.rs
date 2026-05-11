use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Reasons a `blame_requests` row was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlameReason {
    Seed,
    ParentRecurse,
}

impl BlameReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Seed => "seed",
            Self::ParentRecurse => "parent_recurse",
        }
    }
}

/// Discriminator for `file_events.event_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEventType {
    Added,
    Modified,
    Renamed,
    Copied,
    Deleted,
    BinarySkipped,
}

impl FileEventType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
            Self::Deleted => "deleted",
            Self::BinarySkipped => "binary_skipped",
        }
    }
}

/// Discriminator for `lineage_edges.edge_type`. Only `RecurseToParent`
/// has a non-null `next_request_id`; all others are terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineageEdgeType {
    RecurseToParent,
    IntroducedHere,
    RootCommit,
    OlderThanSince,
    MaxDepth,
    BinarySkipped,
}

impl LineageEdgeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecurseToParent => "recurse_to_parent",
            Self::IntroducedHere => "introduced_here",
            Self::RootCommit => "root_commit",
            Self::OlderThanSince => "older_than_since",
            Self::MaxDepth => "max_depth",
            Self::BinarySkipped => "binary_skipped",
        }
    }
}

/// Open a fresh database at `path`, removing any prior file (and its SQLite
/// `-wal` / `-shm` sidecars) so every scan starts from a clean state.
pub fn open_fresh(path: &Path) -> Result<Connection> {
    for victim in [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ] {
        if victim.exists() {
            std::fs::remove_file(&victim)
                .with_context(|| format!("removing {}", victim.display()))?;
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating db parent dir {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("opening database at {}", path.display()))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;
        "#,
    )?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(conn)
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut buf: OsString = path.as_os_str().to_owned();
    buf.push(suffix);
    PathBuf::from(buf)
}

pub const SCHEMA_SQL: &str = r#"
create table if not exists repositories (
  id integer primary key,
  worktree_path text not null,
  git_dir_path text not null,
  remote_url text,
  created_at text not null default current_timestamp,
  unique (git_dir_path)
);

create table if not exists scans (
  id integer primary key,
  repository_id integer not null references repositories(id),
  base_ref text not null,
  base_sha text not null,
  merge_base_sha text not null,
  head_sha text not null,
  max_depth integer not null,
  since_date text not null,
  rename_policy text not null default 'aggressive',
  status text not null default 'running',
  started_at text not null default current_timestamp,
  finished_at text
);

create table if not exists commits (
  repository_id integer not null references repositories(id),
  sha text not null,
  tree_sha text,
  author_name text,
  author_email text,
  authored_at text,
  committer_name text,
  committer_email text,
  committed_at text,
  message text not null,
  primary key (repository_id, sha)
);

create table if not exists commit_parents (
  repository_id integer not null references repositories(id),
  commit_sha text not null,
  parent_sha text not null,
  parent_position integer not null,
  primary key (repository_id, commit_sha, parent_position),
  foreign key (repository_id, commit_sha) references commits(repository_id, sha)
);

create table if not exists files (
  id integer primary key,
  repository_id integer not null references repositories(id),
  first_seen_commit_sha text,
  first_seen_path text,
  created_at text not null default current_timestamp
);

create table if not exists file_versions (
  id integer primary key,
  repository_id integer not null references repositories(id),
  file_id integer references files(id),
  commit_sha text not null,
  path text not null,
  blob_sha text,
  byte_size integer,
  line_count integer,
  is_binary integer not null default 0,
  unique (repository_id, commit_sha, path)
);

create table if not exists file_events (
  id integer primary key,
  repository_id integer not null references repositories(id),
  file_id integer references files(id),
  commit_sha text not null,
  parent_sha text,
  parent_position integer,
  event_type text not null check (
    event_type in ('added', 'modified', 'renamed', 'copied', 'deleted', 'binary_skipped')
  ),
  old_path text,
  new_path text,
  old_blob_sha text,
  new_blob_sha text,
  created_at text not null default current_timestamp
);

create table if not exists diff_hunks (
  id integer primary key,
  repository_id integer not null references repositories(id),
  file_event_id integer references file_events(id),
  commit_sha text not null,
  parent_sha text,
  parent_position integer,
  old_path text,
  new_path text,
  old_start integer,
  old_lines integer,
  new_start integer,
  new_lines integer,
  patch_text text not null
);

create table if not exists seed_ranges (
  id integer primary key,
  scan_id integer not null references scans(id),
  commit_sha text not null,
  path text not null,
  start_line integer not null,
  end_line integer not null,
  diff_hunk_id integer references diff_hunks(id)
);

create table if not exists blame_requests (
  id integer primary key,
  scan_id integer not null references scans(id),
  commit_sha text not null,
  path text not null,
  start_line integer not null,
  end_line integer not null,
  depth integer not null,
  reason text not null check (
    reason in ('seed', 'parent_recurse')
  ),
  status text not null default 'queued',
  created_at text not null default current_timestamp,
  unique (scan_id, commit_sha, path, start_line, end_line)
);

create table if not exists blame_spans (
  id integer primary key,
  request_id integer not null references blame_requests(id),
  repository_id integer not null references repositories(id),
  blamed_commit_sha text not null,
  final_commit_sha text not null,
  final_path text not null,
  final_start_line integer not null,
  origin_path text,
  origin_start_line integer,
  line_count integer not null,
  boundary integer not null default 0,
  diff_hunk_id integer references diff_hunks(id)
);

create table if not exists lineage_edges (
  id integer primary key,
  scan_id integer not null references scans(id),
  from_request_id integer not null references blame_requests(id),
  to_span_id integer not null references blame_spans(id),
  parent_sha text,
  parent_position integer,
  next_request_id integer references blame_requests(id),
  edge_type text not null check (
    edge_type in (
      'recurse_to_parent',
      'introduced_here',
      'root_commit',
      'older_than_since',
      'max_depth',
      'binary_skipped'
    )
  )
);

create index if not exists idx_scans_repo_head on scans(repository_id, head_sha);
create index if not exists idx_scans_repo_base on scans(repository_id, base_ref, head_sha);

create index if not exists idx_commit_parents_parent
  on commit_parents(repository_id, parent_sha);

create index if not exists idx_file_versions_lookup
  on file_versions(repository_id, commit_sha, path);

create index if not exists idx_file_events_commit
  on file_events(repository_id, commit_sha);

create index if not exists idx_file_events_file
  on file_events(repository_id, file_id, commit_sha);

create index if not exists idx_diff_hunks_commit_path
  on diff_hunks(repository_id, commit_sha, new_path, new_start, new_lines);

create index if not exists idx_diff_hunks_parent_path
  on diff_hunks(repository_id, parent_sha, old_path, old_start, old_lines);

create index if not exists idx_seed_ranges_scan_path
  on seed_ranges(scan_id, path, start_line, end_line);

create index if not exists idx_blame_requests_queue
  on blame_requests(scan_id, status, depth);

create index if not exists idx_blame_requests_range
  on blame_requests(scan_id, commit_sha, path, start_line, end_line);

create index if not exists idx_blame_spans_blamed_commit
  on blame_spans(repository_id, blamed_commit_sha);

create index if not exists idx_blame_spans_final_range
  on blame_spans(request_id, final_path, final_start_line, line_count);

create index if not exists idx_lineage_edges_from
  on lineage_edges(scan_id, from_request_id);

create index if not exists idx_lineage_edges_to
  on lineage_edges(scan_id, to_span_id);

create index if not exists idx_lineage_edges_next
  on lineage_edges(scan_id, next_request_id);
"#;

pub fn ensure_repository(
    conn: &Connection,
    worktree_path: &str,
    git_dir_path: &str,
    remote_url: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "insert or ignore into repositories (worktree_path, git_dir_path, remote_url)
         values (?1, ?2, ?3)",
        params![worktree_path, git_dir_path, remote_url],
    )?;
    let id: i64 = conn.query_row(
        "select id from repositories where git_dir_path = ?1",
        params![git_dir_path],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Fields needed to insert a `scans` row.
pub struct InsertScan<'a> {
    pub repository_id: i64,
    pub base_ref: &'a str,
    pub base_sha: &'a str,
    pub merge_base_sha: &'a str,
    pub head_sha: &'a str,
    pub max_depth: u32,
    pub since_date: &'a str,
}

pub fn insert_scan(conn: &Connection, row: InsertScan<'_>) -> Result<i64> {
    conn.execute(
        "insert into scans (
            repository_id, base_ref, base_sha, merge_base_sha, head_sha,
            max_depth, since_date
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.repository_id,
            row.base_ref,
            row.base_sha,
            row.merge_base_sha,
            row.head_sha,
            row.max_depth as i64,
            row.since_date
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finalize_scan(conn: &Connection, scan_id: i64) -> Result<()> {
    conn.execute(
        "update scans set status='complete', finished_at=current_timestamp where id=?1",
        params![scan_id],
    )?;
    Ok(())
}

/// Fields needed to insert a `commits` row.
pub struct UpsertCommit<'a> {
    pub repository_id: i64,
    pub sha: &'a str,
    pub tree_sha: Option<&'a str>,
    pub author_name: Option<&'a str>,
    pub author_email: Option<&'a str>,
    pub authored_at: Option<&'a str>,
    pub committer_name: Option<&'a str>,
    pub committer_email: Option<&'a str>,
    pub committed_at: Option<&'a str>,
    pub message: &'a str,
}

pub fn upsert_commit(conn: &Connection, row: UpsertCommit<'_>) -> Result<()> {
    conn.execute(
        "insert or ignore into commits (
            repository_id, sha, tree_sha, author_name, author_email, authored_at,
            committer_name, committer_email, committed_at, message
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            row.repository_id,
            row.sha,
            row.tree_sha,
            row.author_name,
            row.author_email,
            row.authored_at,
            row.committer_name,
            row.committer_email,
            row.committed_at,
            row.message
        ],
    )?;
    Ok(())
}

pub fn upsert_commit_parent(
    conn: &Connection,
    repository_id: i64,
    commit_sha: &str,
    parent_sha: &str,
    parent_position: i64,
) -> Result<()> {
    conn.execute(
        "insert or ignore into commit_parents (
            repository_id, commit_sha, parent_sha, parent_position
         ) values (?1, ?2, ?3, ?4)",
        params![repository_id, commit_sha, parent_sha, parent_position],
    )?;
    Ok(())
}

/// Fields needed to insert a `file_events` row.
pub struct InsertFileEvent<'a> {
    pub repository_id: i64,
    pub commit_sha: &'a str,
    pub parent_sha: Option<&'a str>,
    pub parent_position: Option<i64>,
    pub event_type: FileEventType,
    pub old_path: Option<&'a str>,
    pub new_path: Option<&'a str>,
    pub old_blob_sha: Option<&'a str>,
    pub new_blob_sha: Option<&'a str>,
}

pub fn insert_file_event(conn: &Connection, row: InsertFileEvent<'_>) -> Result<i64> {
    conn.execute(
        "insert into file_events (
            repository_id, commit_sha, parent_sha, parent_position, event_type,
            old_path, new_path, old_blob_sha, new_blob_sha
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            row.repository_id,
            row.commit_sha,
            row.parent_sha,
            row.parent_position,
            row.event_type.as_str(),
            row.old_path,
            row.new_path,
            row.old_blob_sha,
            row.new_blob_sha
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fields needed to insert a `diff_hunks` row.
pub struct InsertDiffHunk<'a> {
    pub repository_id: i64,
    pub file_event_id: Option<i64>,
    pub commit_sha: &'a str,
    pub parent_sha: Option<&'a str>,
    pub parent_position: Option<i64>,
    pub old_path: Option<&'a str>,
    pub new_path: Option<&'a str>,
    pub old_start: Option<i64>,
    pub old_lines: Option<i64>,
    pub new_start: Option<i64>,
    pub new_lines: Option<i64>,
    pub patch_text: &'a str,
}

pub fn insert_diff_hunk(conn: &Connection, row: InsertDiffHunk<'_>) -> Result<i64> {
    conn.execute(
        "insert into diff_hunks (
            repository_id, file_event_id, commit_sha, parent_sha, parent_position,
            old_path, new_path, old_start, old_lines, new_start, new_lines, patch_text
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            row.repository_id,
            row.file_event_id,
            row.commit_sha,
            row.parent_sha,
            row.parent_position,
            row.old_path,
            row.new_path,
            row.old_start,
            row.old_lines,
            row.new_start,
            row.new_lines,
            row.patch_text
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_seed_range(
    conn: &Connection,
    scan_id: i64,
    commit_sha: &str,
    path: &str,
    start_line: i64,
    end_line: i64,
    diff_hunk_id: Option<i64>,
) -> Result<i64> {
    conn.execute(
        "insert into seed_ranges (
            scan_id, commit_sha, path, start_line, end_line, diff_hunk_id
         ) values (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            scan_id,
            commit_sha,
            path,
            start_line,
            end_line,
            diff_hunk_id
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fields needed to insert (or look up) a blame request.
pub struct InsertBlameRequest<'a> {
    pub scan_id: i64,
    pub commit_sha: &'a str,
    pub path: &'a str,
    pub start_line: i64,
    pub end_line: i64,
    pub depth: u32,
    pub reason: BlameReason,
}

/// Insert (or look up, if it already exists) a blame request. Returns the
/// `blame_requests.id` of the row.
pub fn insert_or_get_blame_request(conn: &Connection, row: InsertBlameRequest<'_>) -> Result<i64> {
    conn.execute(
        "insert or ignore into blame_requests (
            scan_id, commit_sha, path, start_line, end_line, depth, reason
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.scan_id,
            row.commit_sha,
            row.path,
            row.start_line,
            row.end_line,
            row.depth as i64,
            row.reason.as_str()
        ],
    )?;
    let id: i64 = conn.query_row(
        "select id from blame_requests
            where scan_id=?1 and commit_sha=?2 and path=?3 and start_line=?4 and end_line=?5",
        params![
            row.scan_id,
            row.commit_sha,
            row.path,
            row.start_line,
            row.end_line
        ],
        |r| r.get(0),
    )?;
    Ok(id)
}

pub fn pick_next_request(conn: &Connection, scan_id: i64) -> Result<Option<i64>> {
    conn.query_row(
        "select id from blame_requests
            where scan_id=?1 and status='queued'
            order by depth asc, id asc
            limit 1",
        params![scan_id],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn mark_request_complete(conn: &Connection, request_id: i64) -> Result<()> {
    conn.execute(
        "update blame_requests set status='complete' where id=?1",
        params![request_id],
    )?;
    Ok(())
}

/// A row read back from `blame_requests`.
pub struct BlameRequest {
    pub id: i64,
    pub commit_sha: String,
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub depth: u32,
}

pub fn load_request(conn: &Connection, id: i64) -> Result<BlameRequest> {
    conn.query_row(
        "select id, commit_sha, path, start_line, end_line, depth
            from blame_requests where id=?1",
        params![id],
        |r| {
            Ok(BlameRequest {
                id: r.get(0)?,
                commit_sha: r.get(1)?,
                path: r.get(2)?,
                start_line: r.get(3)?,
                end_line: r.get(4)?,
                depth: r.get::<_, i64>(5)? as u32,
            })
        },
    )
    .map_err(Into::into)
}

/// Fields needed to insert a `blame_spans` row.
pub struct InsertBlameSpan<'a> {
    pub request_id: i64,
    pub repository_id: i64,
    pub blamed_commit_sha: &'a str,
    pub final_commit_sha: &'a str,
    pub final_path: &'a str,
    pub final_start_line: i64,
    pub origin_path: Option<&'a str>,
    pub origin_start_line: Option<i64>,
    pub line_count: i64,
    pub boundary: bool,
    pub diff_hunk_id: Option<i64>,
}

pub fn insert_blame_span(conn: &Connection, row: InsertBlameSpan<'_>) -> Result<i64> {
    conn.execute(
        "insert into blame_spans (
            request_id, repository_id, blamed_commit_sha, final_commit_sha,
            final_path, final_start_line, origin_path, origin_start_line,
            line_count, boundary, diff_hunk_id
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            row.request_id,
            row.repository_id,
            row.blamed_commit_sha,
            row.final_commit_sha,
            row.final_path,
            row.final_start_line,
            row.origin_path,
            row.origin_start_line,
            row.line_count,
            row.boundary as i64,
            row.diff_hunk_id
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fields needed to insert a `lineage_edges` row.
pub struct InsertLineageEdge<'a> {
    pub scan_id: i64,
    pub from_request_id: i64,
    pub to_span_id: i64,
    pub parent_sha: Option<&'a str>,
    pub parent_position: Option<i64>,
    pub next_request_id: Option<i64>,
    pub edge_type: LineageEdgeType,
}

pub fn insert_lineage_edge(conn: &Connection, row: InsertLineageEdge<'_>) -> Result<i64> {
    conn.execute(
        "insert into lineage_edges (
            scan_id, from_request_id, to_span_id, parent_sha, parent_position,
            next_request_id, edge_type
         ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.scan_id,
            row.from_request_id,
            row.to_span_id,
            row.parent_sha,
            row.parent_position,
            row.next_request_id,
            row.edge_type.as_str()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub struct ScanSummary {
    pub seed_files: i64,
    pub seed_ranges: i64,
    pub requests_processed: i64,
    pub commits_discovered: i64,
    pub terminal_spans: i64,
}

pub fn summarize(conn: &Connection, scan_id: i64, repository_id: i64) -> Result<ScanSummary> {
    let seed_files: i64 = conn.query_row(
        "select count(distinct path) from seed_ranges where scan_id=?1",
        params![scan_id],
        |r| r.get(0),
    )?;
    let seed_ranges: i64 = conn.query_row(
        "select count(*) from seed_ranges where scan_id=?1",
        params![scan_id],
        |r| r.get(0),
    )?;
    let requests_processed: i64 = conn.query_row(
        "select count(*) from blame_requests where scan_id=?1 and status='complete'",
        params![scan_id],
        |r| r.get(0),
    )?;
    let commits_discovered: i64 = conn.query_row(
        "select count(*) from commits where repository_id=?1",
        params![repository_id],
        |r| r.get(0),
    )?;
    let terminal_spans: i64 = conn.query_row(
        "select count(*) from lineage_edges
            where scan_id=?1 and edge_type<>'recurse_to_parent'",
        params![scan_id],
        |r| r.get(0),
    )?;
    Ok(ScanSummary {
        seed_files,
        seed_ranges,
        requests_processed,
        commits_discovered,
        terminal_spans,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_appends_suffix_for_extension_path() {
        let p = Path::new("/tmp/foo/db.sqlite");
        assert_eq!(sidecar(p, "-wal"), PathBuf::from("/tmp/foo/db.sqlite-wal"));
        assert_eq!(sidecar(p, "-shm"), PathBuf::from("/tmp/foo/db.sqlite-shm"));
    }

    #[test]
    fn sidecar_appends_suffix_when_no_extension() {
        let p = Path::new("histoire");
        assert_eq!(sidecar(p, "-wal"), PathBuf::from("histoire-wal"));
    }
}

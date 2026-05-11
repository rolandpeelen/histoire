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

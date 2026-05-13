---
name: histoire
description: Use the SQLite database produced by `histoire scan` or `histoire trace` to recover historical context — who wrote the lines changed on a branch, or the full lineage behind a specific file:line target.
---

# histoire

`histoire` is a CLI that runs recursive `git blame` and writes the resulting lineage graph to a SQLite database. It has two seed modes:

- **`scan`** — seed from the diff between `HEAD` and a base ref (default `origin/main`). Use when reviewing a PR or branch.
- **`trace`** — seed from a single `path:line(-end)` target at `HEAD`. Use when investigating one specific region of code.

Both modes share the same recursion machinery and the same database schema.

## Use this skill when

- The user asks you to review the changes on the current branch and you want to know what the new code replaced (`scan`).
- The user points at a specific file and line and asks how it got that way (`trace path:line`).
- The user asks who last touched a line, what the previous version looked like, or why a function evolved a certain way.
- You are generating code in a region with non-trivial history and want to avoid undoing earlier intent (e.g. a fix, a workaround, a deliberate API choice).

## Running it

`histoire` must run inside a Git working tree.

```sh
# scan — seed from the merge_base..HEAD diff.
# Defaults: base = origin/main, db = <git-dir>/histoire.sqlite, depth = 5, since = six months ago.
histoire scan

# Explicit base ref (e.g. on a repo where the default branch is master):
histoire scan origin/master

# Tune depth, since, rename threshold:
histoire scan origin/main --max-depth 8 --since 2024-01-01 --rename-threshold 40

# trace — seed from one path:line target at HEAD.
# Defaults: depth = 20, since = twelve months ago.
histoire trace src/main.rs:300
histoire trace src/main.rs:300-320          # line range
histoire trace src/main.rs:300 --max-depth 40 --since 2023-01-01
```

Each run drops and recreates the database, so it always reflects the most recent invocation. Re-run `histoire scan` / `histoire trace` before you query if anything has changed.

If `scan`'s base ref does not exist (e.g. you ran the default `origin/main` on a `master`-only repo), `histoire` warns and writes an empty scan rather than failing. Re-run with the correct base ref.

## Where the database lives

Default path: `<git-dir>/histoire.sqlite` — usually `.git/histoire.sqlite` in a normal worktree. Override with `histoire scan --db <path>`.

Inspect with the `sqlite3` CLI:

```sh
sqlite3 .git/histoire.sqlite ".schema"
sqlite3 .git/histoire.sqlite "SELECT * FROM scans;"
```

## Conceptual model

- A **scan** records one run: base ref, base SHA, merge base, HEAD, max depth, since-date. (For `trace` runs the `base_ref` column is set to `trace:<path>:<start>-<end>` and the base/merge-base SHAs both equal HEAD.)
- The **seed** is the set of starting line-ranges:
  - `scan` mode: every added line-range in the `merge_base..HEAD` diff.
  - `trace` mode: a single range from the `path:line(-end)` target.
- Each seed becomes a depth-0 `blame_requests` row.
- Processing a request runs `git blame` clipped to its range and produces one or more `blame_spans`, each attributed to a single ancestor `blamed_commit_sha`.
- For each span, `histoire` either terminates (`root_commit`, `older_than_since`, `max_depth`, `introduced_here`, `binary_skipped`) or creates a depth-N+1 request against each parent of the blamed commit. The relationship is recorded in `lineage_edges`.
- `file_events` and `diff_hunks` record every delta discovered while walking, including **rename** and **copy** events detected with aggressive similarity matching (default threshold 50; tune with `--rename-threshold`).

## Schema

```sql
{{SCHEMA_DDL}}
```

## Anchor every query on the latest scan

Histoire keeps only the most recent scan (the DB is dropped per run), but the `scan_id` column is everywhere — anchor queries with:

```sql
WHERE scan_id = (SELECT MAX(id) FROM scans)
```

## Useful queries

### Files touched by the current branch
```sql
SELECT DISTINCT path
FROM seed_ranges
WHERE scan_id = (SELECT MAX(id) FROM scans)
ORDER BY path;
```

### Added line-ranges for a specific file
```sql
SELECT start_line, end_line
FROM seed_ranges
WHERE scan_id = (SELECT MAX(id) FROM scans)
  AND path = :path
ORDER BY start_line;
```

### Authors most responsible for the changed lines
```sql
SELECT c.sha,
       c.author_name,
       c.committed_at,
       SUBSTR(c.message, 1, 80) AS title,
       COUNT(bs.id) AS spans
FROM blame_spans bs
JOIN blame_requests br ON br.id = bs.request_id
JOIN commits c
  ON c.repository_id = bs.repository_id
 AND c.sha = bs.blamed_commit_sha
WHERE br.scan_id = (SELECT MAX(id) FROM scans)
GROUP BY c.sha
ORDER BY spans DESC
LIMIT 20;
```

### Lineage of a specific changed range (walk backward)
```sql
-- 1. Find the seed request covering the range:
SELECT id, commit_sha, start_line, end_line
FROM blame_requests
WHERE scan_id = (SELECT MAX(id) FROM scans)
  AND path = :path
  AND start_line <= :line
  AND end_line  >= :line
  AND reason = 'seed';

-- 2. From that request id, walk lineage_edges:
SELECT le.edge_type,
       bs.blamed_commit_sha,
       bs.final_path,
       bs.final_start_line,
       bs.line_count,
       bs.origin_path,
       bs.origin_start_line,
       le.parent_sha,
       le.parent_position,
       le.next_request_id
FROM lineage_edges le
JOIN blame_spans bs ON bs.id = le.to_span_id
WHERE le.from_request_id = :request_id
ORDER BY bs.final_start_line, le.parent_position;
```

Recurse on `next_request_id` to walk further back through history. Edges where `edge_type != 'recurse_to_parent'` are terminals (history ends or the cutoff was hit).

### Patch text for a specific change
```sql
-- All hunks for the seed of a path:
SELECT dh.id, dh.new_start, dh.new_lines, dh.patch_text
FROM seed_ranges sr
JOIN diff_hunks dh ON dh.id = sr.diff_hunk_id
WHERE sr.scan_id = (SELECT MAX(id) FROM scans)
  AND sr.path = :path
ORDER BY dh.new_start;

-- Patch text for an arbitrary hunk id:
SELECT patch_text FROM diff_hunks WHERE id = :hunk_id;
```

### Renames and copies discovered on the branch
```sql
SELECT commit_sha, parent_sha, event_type, old_path, new_path
FROM file_events
WHERE event_type IN ('renamed', 'copied')
ORDER BY commit_sha;
```

### Commits discovered during the scan (with parent links)
```sql
SELECT c.sha,
       SUBSTR(c.message, 1, 60) AS title,
       c.author_name,
       c.committed_at,
       GROUP_CONCAT(cp.parent_sha, ',') AS parents
FROM commits c
LEFT JOIN commit_parents cp
  ON cp.repository_id = c.repository_id
 AND cp.commit_sha = c.sha
GROUP BY c.sha
ORDER BY c.committed_at DESC;
```

## Tips when reviewing or generating code

- `blame_spans` carry both `final_*` (where the line is now, in the request's commit/path) and `origin_*` (where the line lives at the blamed commit, after walking renames). Use `origin_path` to fetch the file from `git show <blamed_commit_sha>:<origin_path>` if you need full context.
- `lineage_edges.edge_type` distinguishes terminal vs recursive edges. Only `recurse_to_parent` has a non-null `next_request_id`.
- Merge commits produce multiple edges per span (one per parent). Disambiguate with `parent_position`.
- The most useful join for "show me everything about a changed file" is `seed_ranges → blame_requests → blame_spans → lineage_edges → commits`.
- If a scan looks empty or stale (e.g. `seed_ranges` count is 0 but the branch obviously has changes), the base ref probably did not resolve. Re-run with the correct one: `histoire scan origin/<branch>`.
- To tell a `scan` database apart from a `trace` database, look at `scans.base_ref` — `trace` runs prefix it with `trace:` and set `base_sha = merge_base_sha = head_sha`.

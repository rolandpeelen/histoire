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

/// Value of the `parent_position` column for `file_events` / `diff_hunks`.
/// `Seed` is the merge-base→HEAD diff that initiates the scan (NULL on disk);
/// `Index(n)` is the n-th parent of a commit visited during recursion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParentPos {
    Seed,
    Index(u32),
}

impl From<ParentPos> for Option<i64> {
    fn from(position: ParentPos) -> Self {
        match position {
            ParentPos::Seed => None,
            ParentPos::Index(index) => Some(i64::from(index)),
        }
    }
}

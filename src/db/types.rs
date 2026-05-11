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

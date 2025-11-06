//! Release state tracking.

use crate::git::{BranchInfo, CommitInfo, TagInfo};

/// State tracking for release operations
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct ReleaseState {
    /// Commit created for this release
    pub(super) release_commit: Option<CommitInfo>,
    /// Tag created for this release
    pub(super) release_tag: Option<TagInfo>,
    /// Branch created for this release
    pub(super) release_branch: Option<BranchInfo>,
    /// Whether commits have been pushed
    pub(super) commits_pushed: bool,
    /// Whether tags have been pushed
    pub(super) tags_pushed: bool,
    /// Whether branch has been pushed
    pub(super) branch_pushed: bool,
    /// Previous HEAD before release (for rollback)
    pub(super) previous_head: Option<String>,
}

//! Git operations and management for release workflows.
//!
//! This module provides comprehensive Git integration using the gix library,
//! offering atomic operations, rollback capabilities, and release coordination.

mod git_adapter;
mod manager;
mod operations;

pub use git_adapter::KodegenGitOperations;
pub use manager::{GitConfig, GitManager, ReleaseResult, RollbackResult};
pub use operations::{
    BranchInfo, CommitInfo, GitOperations, PushInfo, RemoteInfo, ResetType, TagInfo,
    ValidationResult,
};

//! Git manager for coordinating release operations.
//!
//! This module provides high-level Git management for release workflows,
//! coordinating commits, tags, pushes, and rollbacks.

#![allow(dead_code)] // Public API - methods/structs may be used by external consumers

mod config;
mod release;
mod results;
mod state;
mod validation;

pub use config::GitConfig;
pub use results::{BackupPoint, ReleaseResult, RollbackResult, RepositoryStats};
pub use state::ReleaseState;

use crate::error::Result;
use crate::git::{BranchInfo, CommitInfo, GitOperations, KodegenGitOperations, ValidationResult};
use semver::Version;
use std::cell::RefCell;
use std::path::Path;

use release::ReleaseOperations;
use validation::ValidationOperations;

/// High-level Git manager for release operations
#[derive(Debug)]
pub struct GitManager {
    /// Underlying Git repository
    repository: KodegenGitOperations,
    /// Configuration for Git operations
    config: GitConfig,
    /// Release state tracking
    release_state: RefCell<ReleaseState>,
}

impl GitManager {
    /// Create a new Git manager for the given repository path
    pub async fn new<P: AsRef<Path>>(repo_path: P) -> Result<Self> {
        let repository = KodegenGitOperations::open(repo_path).await?;
        let config = GitConfig::default();
        let release_state = RefCell::new(ReleaseState::default());

        Ok(Self {
            repository,
            config,
            release_state,
        })
    }

    /// Create a Git manager with custom configuration
    pub async fn with_config<P: AsRef<Path>>(repo_path: P, config: GitConfig) -> Result<Self> {
        let repository = KodegenGitOperations::open(repo_path).await?;
        let release_state = RefCell::new(ReleaseState::default());

        Ok(Self {
            repository,
            config,
            release_state,
        })
    }

    /// Perform a complete automated release operation
    pub async fn perform_release(
        &self,
        version: &Version,
        push_to_remote: bool,
    ) -> Result<ReleaseResult> {
        let ops = ReleaseOperations {
            repository: &self.repository,
            config: &self.config,
            release_state: &self.release_state,
        };
        ops.perform_release(version, push_to_remote).await
    }

    /// Rollback a release operation
    pub async fn rollback_release(&self) -> Result<RollbackResult> {
        let ops = ReleaseOperations {
            repository: &self.repository,
            config: &self.config,
            release_state: &self.release_state,
        };
        ops.rollback_release().await
    }

    /// Check if working directory is clean
    pub async fn is_clean(&self) -> Result<bool> {
        self.repository.is_working_directory_clean().await
    }

    /// Get current branch information
    pub async fn current_branch(&self) -> Result<BranchInfo> {
        self.repository.get_current_branch().await
    }

    /// Get recent commit history
    pub async fn recent_commits(&self, count: usize) -> Result<Vec<CommitInfo>> {
        self.repository.get_recent_commits(count).await
    }

    /// Check if a version tag already exists
    pub async fn version_tag_exists(&self, version: &Version) -> Result<bool> {
        let ops = ValidationOperations {
            repository: &self.repository,
        };
        ops.version_tag_exists(version).await
    }

    /// Check if a release branch already exists (local)
    pub async fn release_branch_exists(&self, version: &Version) -> Result<bool> {
        let ops = ValidationOperations {
            repository: &self.repository,
        };
        ops.release_branch_exists(version).await
    }

    /// Check if a release branch exists on remote
    pub async fn remote_release_branch_exists(&self, version: &Version) -> Result<bool> {
        let ops = ValidationOperations {
            repository: &self.repository,
        };
        ops.remote_release_branch_exists(version).await
    }

    /// Clean up existing tag (both local and remote)
    pub async fn cleanup_existing_tag(&self, version: &Version) -> Result<()> {
        let ops = ValidationOperations {
            repository: &self.repository,
        };
        ops.cleanup_existing_tag(version).await
    }

    /// Clean up existing release branch (both local and remote)
    pub async fn cleanup_existing_branch(&self, version: &Version) -> Result<()> {
        let ops = ValidationOperations {
            repository: &self.repository,
        };
        ops.cleanup_existing_branch(version).await
    }

    /// Get remote information
    pub async fn remotes(&self) -> Result<Vec<crate::git::RemoteInfo>> {
        self.repository.get_remotes().await
    }

    /// Validate repository for release operations
    pub async fn validate(&self) -> Result<ValidationResult> {
        self.repository.validate_release_readiness().await
    }

    /// Get configuration
    pub fn config(&self) -> &GitConfig {
        &self.config
    }

    /// Update configuration
    pub fn set_config(&mut self, config: GitConfig) {
        self.config = config;
    }

    /// Check if there's an active release
    pub fn has_active_release(&self) -> bool {
        let state = self.release_state.borrow();
        state.release_commit.is_some() || state.release_tag.is_some()
    }

    /// Get current release state
    pub fn release_state(&self) -> ReleaseState {
        self.release_state.borrow().clone()
    }

    /// Clear release state (call after successful completion)
    pub fn clear_release_state(&self) {
        *self.release_state.borrow_mut() = ReleaseState::default();
    }

    /// Create a backup point before starting operations
    pub async fn create_backup_point(&mut self) -> Result<BackupPoint> {
        let ops = ValidationOperations {
            repository: &self.repository,
        };
        ops.create_backup_point().await
    }

    /// Restore from a backup point
    ///
    /// SAFE RESTORE: Checks out the branch instead of using destructive reset
    /// All work is preserved - this just switches to the backup branch
    pub async fn restore_from_backup(&self, backup: &BackupPoint) -> Result<()> {
        // Safe: Just checkout the branch, don't reset
        self.repository.checkout_branch(&backup.branch_name).await
    }

    /// Get Git repository statistics
    pub async fn get_repository_stats(&self) -> Result<RepositoryStats> {
        let ops = ValidationOperations {
            repository: &self.repository,
        };
        ops.get_repository_stats().await
    }
}

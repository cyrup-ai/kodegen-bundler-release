//! Validation and helper operations.

use crate::error::Result;
use crate::git::{GitOperations, KodegenGitOperations};
use semver::Version;

use super::results::{BackupPoint, RepositoryStats};

/// Validation operations for GitManager
pub(super) struct ValidationOperations<'a> {
    pub(super) repository: &'a KodegenGitOperations,
}

impl<'a> ValidationOperations<'a> {
    /// Check if a version tag already exists
    pub async fn version_tag_exists(&self, version: &Version) -> Result<bool> {
        let tag_name = format!("v{}", version);
        self.repository.tag_exists(&tag_name).await
    }

    /// Check if a release branch already exists (local)
    pub async fn release_branch_exists(&self, version: &Version) -> Result<bool> {
        let branch_name = format!("v{}", version);
        self.repository.branch_exists(&branch_name).await
    }

    /// Check if a release branch exists on remote
    pub async fn remote_release_branch_exists(&self, version: &Version) -> Result<bool> {
        let branch_name = format!("v{}", version);
        self.repository.remote_branch_exists("origin", &branch_name).await
    }

    /// Clean up existing tag (both local and remote)
    ///
    /// Deletes the tag locally and from the remote if it exists.
    /// Safe to call even if tag doesn't exist - will silently succeed.
    pub async fn cleanup_existing_tag(&self, version: &Version) -> Result<()> {
        let tag_name = format!("v{}", version);
        
        // Check if tag exists locally
        if self.repository.tag_exists(&tag_name).await? {
            // Delete with delete_remote=true to cleanup both local and remote
            self.repository.delete_tag(&tag_name, true).await?;
        }
        
        Ok(())
    }

    /// Clean up existing release branch (both local and remote)
    ///
    /// Deletes the branch locally and from the remote if it exists.
    /// Safe to call even if branch doesn't exist - will silently succeed.
    pub async fn cleanup_existing_branch(&self, version: &Version) -> Result<()> {
        let branch_name = format!("v{}", version);
        
        // Delete remote branch first (if exists)
        if self.repository.remote_branch_exists("origin", &branch_name).await? {
            self.repository.delete_remote_branch("origin", &branch_name).await?;
        }
        
        // Delete local branch (if exists)
        if self.repository.branch_exists(&branch_name).await? {
            self.repository.delete_branch(&branch_name, false).await?;
        }
        
        Ok(())
    }

    /// Create a backup point before starting operations
    pub async fn create_backup_point(&self) -> Result<BackupPoint> {
        let current_branch = self.repository.get_current_branch().await?;
        let recent_commits = self.repository.get_recent_commits(5).await?;

        Ok(BackupPoint {
            branch_name: current_branch.name,
            commit_hash: current_branch.commit_hash,
            timestamp: chrono::Utc::now(),
            recent_commits,
        })
    }

    /// Get Git repository statistics
    pub async fn get_repository_stats(&self) -> Result<RepositoryStats> {
        let current_branch = self.repository.get_current_branch().await?;
        let is_clean = self.repository.is_working_directory_clean().await?;
        let remotes = self.repository.get_remotes().await?;
        let recent_commits = self.repository.get_recent_commits(10).await?;

        Ok(RepositoryStats {
            current_branch: current_branch.name,
            is_clean,
            remote_count: remotes.len(),
            recent_commit_count: recent_commits.len(),
            has_upstream: current_branch.upstream.is_some(),
        })
    }
}

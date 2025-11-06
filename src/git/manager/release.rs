//! Core release operations implementation.

use crate::error::{GitError, Result};
use crate::git::{GitOperations, KodegenGitOperations};
use semver::Version;
use std::sync::Mutex;

use super::config::GitConfig;
use super::results::{ReleaseResult, RollbackResult};
use super::state::ReleaseState;

/// Release operations for GitManager
pub(super) struct ReleaseOperations<'a> {
    pub(super) repository: &'a KodegenGitOperations,
    pub(super) config: &'a GitConfig,
    pub(super) release_state: &'a Mutex<ReleaseState>,
}

impl<'a> ReleaseOperations<'a> {
    /// Perform a complete automated release operation
    ///
    /// This method handles the full release workflow from ANY starting state:
    /// 1. Commits any uncommitted work on the current branch (never leaves work dirty)
    /// 2. Switches to main branch
    /// 3. Merges work from the original branch (ONLY if not a release branch)
    /// 4. Creates a new release branch matching the bumped version
    /// 5. Commits version changes on the release branch
    /// 6. Creates version tag
    /// 7. Pushes release branch with tags to remote (if requested)
    /// 8. ALWAYS returns to main branch (using checkout, never reset)
    pub async fn perform_release(
        &self,
        version: &Version,
        push_to_remote: bool,
    ) -> Result<ReleaseResult> {
        let start_time = std::time::Instant::now();

        // Step 1: Save starting state - record current branch name
        let original_branch = self.repository.get_current_branch().await?;
        let original_branch_name = original_branch.name.clone();

        // Step 2: Commit work-in-progress if working directory is dirty
        // This ensures we NEVER have uncommitted changes that could be lost
        let is_clean = self.repository.is_working_directory_clean().await?;
        if !is_clean {
            match self
                .repository
                .commit_all_changes("WIP: Auto-commit before release")
                .await
            {
                Ok(_wip_commit) => {
                    // Auto-commit of uncommitted changes succeeded
                }
                Err(e) => {
                    // Failed to auto-commit uncommitted changes - try to return to main and report status
                    let recovery_instructions = self.get_recovery_instructions().await;
                    return Err(GitError::CommitFailed {
                        reason: format!(
                            "Failed to commit work-in-progress: {}. Recovery: {}",
                            e, recovery_instructions
                        ),
                    }
                    .into());
                }
            }
        }

        // Step 3: Switch to main branch
        if let Err(e) = self.repository.checkout_branch("main").await {
            // Don't retry the same failing operation - just report current state
            let current_state = self.get_current_state_for_error().await;
            return Err(GitError::BranchOperationFailed {
                reason: format!(
                    "Failed to checkout main branch: {}. Current state: {}",
                    e, current_state
                ),
            }
            .into());
        }

        // Step 4: Merge work from original branch (ONLY if not already on main AND not a release branch)
        // Release branches (starting with 'v') should NOT be merged back into main
        let is_release_branch = original_branch_name.starts_with('v');
        if original_branch_name != "main" && !is_release_branch {
            match self.repository.merge_branch(&original_branch_name).await {
                Ok(()) => {
                    // Merge succeeded
                }
                Err(e) => {
                    // Merge failed - abort it to clean up
                    if let Err(abort_err) = self.repository.abort_merge().await {
                        eprintln!(
                            "Warning: Failed to abort merge: {}. \
                             Repository may be in inconsistent state.",
                            abort_err
                        );
                    }
                    
                    let recovery_instructions = self.get_recovery_instructions().await;
                    return Err(GitError::BranchOperationFailed {
                        reason: format!(
                            "Failed to merge branch '{}' into main: {}. \
                             Merge has been aborted. Recovery: {}",
                            original_branch_name, e, recovery_instructions
                        ),
                    }
                    .into());
                }
            }
        }

        // Step 5: Create release branch matching the bumped version
        let release_branch = match self.repository.create_release_branch(version).await {
            Ok(branch) => branch,
            Err(e) => {
                let recovery_instructions = self.get_recovery_instructions().await;
                return Err(GitError::BranchOperationFailed {
                    reason: format!(
                        "Failed to create release branch: {}. Recovery: {}",
                        e, recovery_instructions
                    ),
                }
                .into());
            }
        };

        // Track the created branch in release state for rollback
        self.release_state
            .lock()
            .map_err(|_| GitError::BranchOperationFailed {
                reason: "Failed to acquire lock on release state".to_string(),
            })?
            .release_branch = Some(release_branch.clone());

        // Step 6: Commit version changes on release branch
        let commit_message = self.config.generate_commit_message(version);
        let commit = match self
            .repository
            .create_release_commit(version, Some(commit_message))
            .await
        {
            Ok(c) => c,
            Err(e) => {
                let recovery_instructions = self.get_recovery_instructions().await;
                return Err(GitError::CommitFailed {
                    reason: format!(
                        "Failed to create release commit: {}. Recovery: {}",
                        e, recovery_instructions
                    ),
                }
                .into());
            }
        };
        self.release_state
            .lock()
            .map_err(|_| GitError::CommitFailed {
                reason: "Failed to acquire lock on release state".to_string(),
            })?
            .release_commit = Some(commit.clone());

        // Step 7: Create version tag on release branch
        let tag_message = self.config.generate_tag_message(version);
        let tag = match self
            .repository
            .create_version_tag(version, Some(tag_message))
            .await
        {
            Ok(t) => t,
            Err(e) => {
                let recovery_instructions = self.get_recovery_instructions().await;
                return Err(GitError::CommitFailed {
                    reason: format!(
                        "Failed to create version tag: {}. Recovery: {}",
                        e, recovery_instructions
                    ),
                }
                .into());
            }
        };
        self.release_state
            .lock()
            .map_err(|_| GitError::CommitFailed {
                reason: "Failed to acquire lock on release state".to_string(),
            })?
            .release_tag = Some(tag.clone());

        // Step 8: Push release branch with tags to remote (if requested)
        let release_branch_push_info = if push_to_remote {
            let branch_name = format!("v{}", version);
            match self
                .repository
                .push_branch_with_tags(&branch_name, Some(&self.config.default_remote))
                .await
            {
                Ok(push_info) => {
                    let mut state = self.release_state
                        .lock()
                        .map_err(|_| GitError::PushFailed {
                            reason: "Failed to acquire lock on release state".to_string(),
                        })?;
                    state.tags_pushed = true;
                    state.branch_pushed = true;
                    Some(push_info)
                }
                Err(e) => {
                    let recovery_instructions = self.get_recovery_instructions().await;
                    return Err(GitError::PushFailed {
                        reason: format!(
                            "Failed to push release branch: {}. Local changes preserved. Recovery: {}",
                            e, recovery_instructions
                        ),
                    }
                    .into());
                }
            }
        } else {
            None
        };

        // Step 9: ALWAYS return to main branch
        self.repository.checkout_branch("main").await?;

        let duration = start_time.elapsed();

        Ok(ReleaseResult {
            version: version.clone(),
            commit,
            tag,
            push_info: None, // No longer pushing main branch separately
            release_branch,
            release_branch_push_info,
            duration,
        })
    }

    /// Rollback a release operation
    ///
    /// SAFE ROLLBACK: Only deletes tags, never uses git reset
    /// All work is preserved in commits - nothing is destroyed
    pub async fn rollback_release(&self) -> Result<RollbackResult> {
        let start_time = std::time::Instant::now();
        let mut rolled_back_operations = Vec::new();
        let mut warnings = Vec::new();
        let mut success = true;

        // Rollback in reverse order of operations

        // 1. Delete remote tag if it was pushed
        let tag_name = {
            match self.release_state.lock() {
                Ok(state) => {
                    if state.tags_pushed && state.release_tag.is_some() {
                        state.release_tag.as_ref().map(|t| t.name.clone())
                    } else {
                        None
                    }
                }
                Err(poisoned) => {
                    let state = poisoned.into_inner();
                    if state.tags_pushed && state.release_tag.is_some() {
                        state.release_tag.as_ref().map(|t| t.name.clone())
                    } else {
                        None
                    }
                }
            }
        };
        
        if let Some(tag_name) = tag_name {
            match self.repository.delete_tag(&tag_name, true).await {
                Ok(()) => {
                    rolled_back_operations.push(format!("Deleted remote tag {}", tag_name));
                }
                Err(e) => {
                    warnings.push(format!(
                        "Failed to delete remote tag {}: {}",
                        tag_name, e
                    ));
                }
            }
        }

        // 2. Delete local tag
        let tag_name = {
            match self.release_state.lock() {
                Ok(state) => state.release_tag.as_ref().map(|t| t.name.clone()),
                Err(poisoned) => poisoned.into_inner().release_tag.as_ref().map(|t| t.name.clone()),
            }
        };
        
        if let Some(tag_name) = tag_name {
            match self.repository.delete_tag(&tag_name, false).await {
                Ok(()) => {
                    rolled_back_operations.push(format!("Deleted local tag {}", tag_name));
                }
                Err(e) => {
                    warnings.push(format!(
                        "Failed to delete local tag {}: {}",
                        tag_name, e
                    ));
                    success = false;
                }
            }
        }

        // 3. Delete remote branch if it was pushed
        let branch_name = {
            match self.release_state.lock() {
                Ok(state) => {
                    if state.branch_pushed && state.release_branch.is_some() {
                        state.release_branch.as_ref().map(|b| b.name.clone())
                    } else {
                        None
                    }
                }
                Err(poisoned) => {
                    let state = poisoned.into_inner();
                    if state.branch_pushed && state.release_branch.is_some() {
                        state.release_branch.as_ref().map(|b| b.name.clone())
                    } else {
                        None
                    }
                }
            }
        };
        
        if let Some(branch_name) = branch_name {
            match self.repository.delete_remote_branch("origin", &branch_name).await {
                Ok(()) => {
                    rolled_back_operations.push(format!(
                        "Deleted remote branch {}", 
                        branch_name
                    ));
                }
                Err(e) => {
                    warnings.push(format!(
                        "Failed to delete remote branch {}: {}",
                        branch_name, e
                    ));
                }
            }
        }

        // 4. Return to main branch (safe - no reset, just checkout)
        // NOTE: This MUST happen before deleting local branch because
        // delete_branch has a safety check preventing deletion of current branch
        match self.repository.checkout_branch("main").await {
            Ok(()) => {
                rolled_back_operations.push("Returned to main branch".to_string());
            }
            Err(e) => {
                warnings.push(format!("Failed to checkout main branch: {}", e));
            }
        }

        // 5. Delete local branch (safe now because we're on main)
        let branch_name = {
            match self.release_state.lock() {
                Ok(state) => state.release_branch.as_ref().map(|b| b.name.clone()),
                Err(poisoned) => poisoned.into_inner().release_branch.as_ref().map(|b| b.name.clone()),
            }
        };
        
        if let Some(branch_name) = branch_name {
            match self.repository.delete_branch(&branch_name, false).await {
                Ok(()) => {
                    rolled_back_operations.push(format!(
                        "Deleted local branch {}", 
                        branch_name
                    ));
                }
                Err(e) => {
                    warnings.push(format!(
                        "Failed to delete local branch {}: {}",
                        branch_name, e
                    ));
                    success = false;
                }
            }
        }

        // Clear release state
        match self.release_state.lock() {
            Ok(mut state) => *state = ReleaseState::default(),
            Err(poisoned) => *poisoned.into_inner() = ReleaseState::default(),
        }

        let duration = start_time.elapsed();

        Ok(RollbackResult {
            success,
            rolled_back_operations,
            warnings,
            duration,
        })
    }

    /// Get recovery instructions for error reporting
    /// 
    /// Returns a formatted message describing the current repository state
    /// and instructions for manual recovery. Does NOT perform any git operations.
    /// 
    /// Used by error handlers to provide context and recovery steps after
    /// failed operations leave the repository in an intermediate state.
    async fn get_recovery_instructions(&self) -> String {
        // Don't attempt checkout - just report current state
        match self.repository.get_current_branch().await {
            Ok(branch_info) => {
                format!(
                    "Release operation failed. Repository is currently on branch '{}'. \
                     Manual intervention may be required to return to main. \
                     Run 'git checkout main' to return to main branch.",
                    branch_info.name
                )
            }
            Err(_) => {
                "Release operation failed. Cannot determine current branch. \
                 Manual intervention required. Run 'git status' to check repository state."
                    .to_string()
            }
        }
    }

    /// Get current repository state for error reporting
    /// 
    /// Used when we don't want to attempt recovery (e.g., when checkout main itself failed)
    /// but need to report where the repository currently is.
    async fn get_current_state_for_error(&self) -> String {
        match self.repository.get_current_branch().await {
            Ok(branch_info) => {
                format!("repository is currently on branch '{}'", branch_info.name)
            }
            Err(_) => {
                "repository state unknown (could not determine current branch)".to_string()
            }
        }
    }
}

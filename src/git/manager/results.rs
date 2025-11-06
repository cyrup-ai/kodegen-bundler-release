//! Result types for Git operations.

use crate::git::{BranchInfo, CommitInfo, PushInfo, TagInfo};
use semver::Version;

/// Result of a complete release operation
#[derive(Debug, Clone)]
pub struct ReleaseResult {
    /// Version that was released
    pub version: Version,
    /// Commit information
    pub commit: CommitInfo,
    /// Tag information
    pub tag: TagInfo,
    /// Push information for main branch (if pushed)
    pub push_info: Option<PushInfo>,
    /// Release branch information
    pub release_branch: BranchInfo,
    /// Push information for release branch (if pushed)
    pub release_branch_push_info: Option<PushInfo>,
    /// Duration of the operation
    pub duration: std::time::Duration,
}

impl ReleaseResult {
    /// Format result for display
    pub fn format_result(&self) -> String {
        let mut result = format!("🎉 Release v{} completed successfully!\n", self.version);
        result.push_str(&format!(
            "📦 Commit: {} ({})\n",
            self.commit.short_hash, self.commit.message
        ));
        result.push_str(&format!(
            "🌿 Release Branch: {}\n",
            self.release_branch.name
        ));
        result.push_str(&format!(
            "🏷️  Tag: {} (on branch {})\n",
            self.tag.name, self.release_branch.name
        ));

        if let Some(ref push_info) = self.push_info {
            result.push_str(&format!(
                "📤 Pushed main to {}: {} commits\n",
                push_info.remote_name, push_info.commits_pushed
            ));
        }

        if let Some(ref branch_push_info) = self.release_branch_push_info {
            result.push_str(&format!(
                "📤 Pushed {} to {}: {} commits, {} tags\n",
                self.release_branch.name,
                branch_push_info.remote_name,
                branch_push_info.commits_pushed,
                branch_push_info.tags_pushed
            ));
        }

        result.push_str(&format!(
            "⏱️  Duration: {:.2}s\n",
            self.duration.as_secs_f64()
        ));

        result
    }
}

/// Result of a rollback operation
#[derive(Debug, Clone)]
pub struct RollbackResult {
    /// Whether rollback was successful
    pub success: bool,
    /// Operations that were rolled back
    pub rolled_back_operations: Vec<String>,
    /// Any warnings during rollback
    pub warnings: Vec<String>,
    /// Duration of the rollback
    pub duration: std::time::Duration,
}

impl RollbackResult {
    /// Format rollback result for display
    pub fn format_result(&self) -> String {
        let status = if self.success { "✅" } else { "⚠️" };
        let mut result = format!("{} Rollback completed\n", status);

        if !self.rolled_back_operations.is_empty() {
            result.push_str("🔄 Operations rolled back:\n");
            for op in &self.rolled_back_operations {
                result.push_str(&format!("  - {}\n", op));
            }
        }

        if !self.warnings.is_empty() {
            result.push_str("⚠️  Warnings:\n");
            for warning in &self.warnings {
                result.push_str(&format!("  - {}\n", warning));
            }
        }

        result.push_str(&format!(
            "⏱️  Duration: {:.2}s\n",
            self.duration.as_secs_f64()
        ));

        result
    }
}

/// Backup point for repository state
#[derive(Debug, Clone)]
pub struct BackupPoint {
    /// Branch name at backup time
    pub branch_name: String,
    /// Commit hash at backup time
    pub commit_hash: String,
    /// Timestamp when backup was created
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Recent commits for reference
    pub recent_commits: Vec<CommitInfo>,
}

/// Repository statistics
#[derive(Debug, Clone)]
pub struct RepositoryStats {
    /// Current branch name
    pub current_branch: String,
    /// Whether working directory is clean
    pub is_clean: bool,
    /// Number of configured remotes
    pub remote_count: usize,
    /// Number of recent commits
    pub recent_commit_count: usize,
    /// Whether current branch has upstream
    pub has_upstream: bool,
}

impl RepositoryStats {
    /// Format stats for display
    pub fn format_stats(&self) -> String {
        let clean_status = if self.is_clean {
            "✅ Clean"
        } else {
            "❌ Dirty"
        };
        let upstream_status = if self.has_upstream {
            "✅ Has upstream"
        } else {
            "⚠️ No upstream"
        };

        format!(
            "📊 Repository Stats:\n\
             Branch: {} ({})\n\
             Working Directory: {}\n\
             Remotes: {}\n\
             Recent Commits: {}\n\
             Upstream: {}",
            self.current_branch,
            clean_status,
            clean_status,
            self.remote_count,
            self.recent_commit_count,
            upstream_status
        )
    }
}

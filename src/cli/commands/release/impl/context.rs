//! Context structure for executing release phases with all required dependencies.

use crate::cli::RuntimeConfig;
use crate::git::GitManager;

/// Context for executing release phases with all required dependencies
pub struct ReleasePhaseContext<'a> {
    /// Temporary directory for isolated execution
    pub release_clone_path: &'a std::path::Path,
    /// Binary name to build and release
    pub binary_name: &'a str,
    /// Target version for this release
    pub new_version: &'a semver::Version,
    /// Runtime configuration for output and settings
    pub config: &'a RuntimeConfig,
    /// Git manager for version control operations
    pub git_manager: &'a GitManager,
    /// GitHub manager for release and artifact management
    pub github_manager: &'a crate::github::GitHubReleaseManager,
    /// GitHub repository owner
    pub github_owner: &'a str,
    /// GitHub repository name
    pub github_repo_name: &'a str,
}

//! Context structure for executing release phases with all required dependencies.

use crate::cli::RuntimeConfig;
use crate::git::GitManager;

use super::super::ReleaseOptions;

/// Context for executing release phases with all required dependencies
pub struct ReleasePhaseContext<'a> {
    /// Temporary directory for isolated execution
    pub temp_dir: &'a std::path::Path,
    /// Package metadata from Cargo.toml
    pub metadata: &'a crate::metadata::PackageMetadata,
    /// Binary name to build and release
    pub binary_name: &'a str,
    /// Target version for this release
    pub new_version: &'a semver::Version,
    /// Runtime configuration for output and settings
    pub config: &'a RuntimeConfig,
    /// Release-specific options (bump type, push behavior, etc.)
    pub options: &'a ReleaseOptions,
    /// Git manager for version control operations
    pub git_manager: &'a GitManager,
    /// GitHub manager for release and artifact management
    pub github_manager: &'a crate::github::GitHubReleaseManager,
    /// GitHub repository owner
    pub owner: &'a str,
    /// GitHub repository name
    pub repo: &'a str,
}

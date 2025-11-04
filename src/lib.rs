//! # Cyrup Release
//!
//! Production-quality release management for Rust workspaces.
//!
//! This crate provides atomic release operations with proper error handling,
//! automatic internal dependency version synchronization, and rollback capabilities
//! including crate yanking for published packages.
//!
//! ## Features
//!
//! - **Atomic Operations**: All release steps succeed or all rollback
//! - **Version Synchronization**: Automatic internal dependency version management  
//! - **Git Integration**: Pure Rust git operations using gix (no CLI dependencies)
//! - **Resume Capability**: Continue interrupted releases from checkpoints
//! - **Rollback Support**: Undo git operations and yank published crates
//! - **Dependency Ordering**: Publish packages in correct dependency order
//!
//! ## Usage
//!
//! ```bash
//! cyrup_release patch          # Bump patch version and publish
//! cyrup_release minor --dry    # Dry run minor version bump
//! cyrup_release rollback       # Rollback failed release
//! cyrup_release resume         # Resume interrupted release
//! ```

// SECURITY: Use deny instead of forbid to allow cli/docker.rs to use unsafe code
// for libc::getuid() and libc::getgid() calls (required for Docker container security)
#![deny(unsafe_code)]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

// Core modules
pub mod bundler;
pub mod cli;
pub mod error;
pub mod git;
pub mod github;
pub mod metadata;
pub mod publish;
pub mod source;
pub mod state;
pub mod version;
pub mod workspace;

// Re-export main types for public API
pub use bundler::{BundleSettings, BundledArtifact, Bundler, PackageType};
pub use cli::Args;
pub use error::{CliError, ReleaseError, Result};
pub use git::{GitManager, GitOperations};
pub use publish::Publisher;
pub use state::{ReleaseState, StateManager};
pub use version::{VersionBump, VersionManager};
pub use workspace::{DependencyGraph, PublishOrder, WorkspaceInfo};

use std::path::PathBuf;

/// Configuration for release operations
#[derive(Debug, Clone)]
pub struct ReleaseConfig {
    /// Skip workspace validation
    pub skip_validation: bool,
    /// Allow dirty git working directory
    pub allow_dirty: bool,
    /// Skip pushing to remote
    pub no_push: bool,
    /// Registry to publish to (default: crates.io)
    pub registry: Option<String>,
    /// Delay between package publishes (seconds)
    pub package_delay: u64,
    /// Maximum retry attempts for publish
    pub max_retries: usize,
    /// Operation timeout (seconds)
    pub timeout: u64,
    /// Create GitHub release
    pub github_release: bool,
    /// GitHub repository (owner/repo)
    pub github_repo: Option<String>,
    /// Create draft GitHub release
    pub github_draft: bool,
    /// Path to release notes file
    pub release_notes: Option<PathBuf>,
    /// Create platform bundles
    pub with_bundles: bool,
    /// Upload bundles to GitHub release
    pub upload_bundles: bool,
    /// Keep temporary clone for debugging (don't auto-cleanup)
    pub keep_temp: bool,
}

impl Default for ReleaseConfig {
    fn default() -> Self {
        Self {
            skip_validation: false,
            allow_dirty: false,
            no_push: false,
            registry: None,
            package_delay: 5,
            max_retries: 3,
            timeout: 300,
            github_release: true, // Always create GitHub releases unless --no-github-release
            github_repo: None,
            github_draft: false,
            release_notes: None,
            with_bundles: true, // Always create platform bundles unless --no-bundles
            upload_bundles: true, // Always upload bundles unless --no-upload-bundles
            keep_temp: false,
        }
    }
}

/// Result of a release operation
#[derive(Debug, Clone)]
pub struct ReleaseResult {
    /// New version number
    pub version: semver::Version,
    /// List of successfully published packages
    pub published_packages: Vec<String>,
    /// Git commit SHA (if committed)
    pub git_commit: Option<String>,
    /// Git tag name (if tagged)
    pub git_tag: Option<String>,
    /// GitHub release URL (if created)
    pub github_release: Option<String>,
    /// Number of artifacts signed
    pub artifacts_signed: usize,
    /// Number of bundles created
    pub bundles_created: usize,
}

/// Result of rollback operation
#[derive(Debug, Clone)]
pub struct RollbackResult {
    /// List of packages that were yanked
    pub packages_yanked: Vec<String>,
    /// Whether git operations were reverted
    pub git_reverted: bool,
    /// Whether GitHub release was deleted
    pub github_deleted: bool,
}

// Historical note: Legacy helper functions and ReleaseManager implementation
// were removed to reduce code bloat (785 lines deleted).
// Active implementations are in:
//   - cli/commands/release/impl.rs (release logic)
//   - cli/commands/temp_clone.rs (clone helpers)
//   - cli/commands/helpers.rs (bundling and GitHub helpers)
// See git history for original code if needed for reference.

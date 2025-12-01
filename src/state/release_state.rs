//! Release state tracking and serialization.
#![allow(dead_code)]

use crate::error::{Result, StateError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current version of the state format
pub const STATE_FORMAT_VERSION: u32 = 2;

/// Complete release operation state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseState {
    /// Version of the state format
    pub format_version: u32,
    /// Save operation version (incremented on every save)
    pub save_version: u64,
    /// Unique ID for this release operation
    pub release_id: String,
    /// Version being released (read from Cargo.toml)
    pub release_version: semver::Version,
    /// Timestamp when release started
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Timestamp when release was last updated
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Current phase of the release
    pub current_phase: ReleasePhase,
    /// Checkpoints passed during release
    pub checkpoints: Vec<ReleaseCheckpoint>,
    /// GitHub release state
    pub github_state: Option<GitHubState>,
    /// Any errors encountered during release
    pub errors: Vec<ReleaseError>,
    /// Release configuration
    pub config: ReleaseConfig,
}

/// Phase of the release operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ReleasePhase {
    /// Initial validation and preparation
    Validation,
    /// GitHub release creation
    GitHubRelease,
    /// Building release binaries
    Building,
    /// Creating platform bundles
    Bundling,
    /// Uploading artifacts
    Uploading,
    /// GitHub release publishing (remove draft status)
    GitHubPublish,
    /// Release completed successfully
    Completed,
    /// Release failed
    Failed,
}

/// Checkpoint in the release process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseCheckpoint {
    /// Checkpoint name
    pub name: String,
    /// Phase this checkpoint belongs to
    pub phase: ReleasePhase,
    /// Timestamp when checkpoint was reached
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Any data associated with this checkpoint
    pub data: Option<serde_json::Value>,
}

/// GitHub release state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubState {
    /// Repository owner
    pub owner: String,
    /// Repository name
    pub repo: String,
    /// Release ID
    pub release_id: Option<u64>,
    /// Release URL
    pub html_url: Option<String>,
    /// Whether this was a draft
    pub draft: bool,
    /// Whether this was a prerelease
    pub prerelease: bool,
    /// Uploaded artifact filenames (for resume capability)
    #[serde(default)]
    pub uploaded_artifacts: Vec<String>,
}

/// Error encountered during release
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseError {
    /// Error message
    pub message: String,
    /// Phase where error occurred
    pub phase: ReleasePhase,
    /// Timestamp when error occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Whether this error is recoverable
    pub recoverable: bool,
    /// Additional context
    pub context: Option<String>,
}

/// Release configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseConfig {
    /// Whether to perform dry run
    #[serde(default)]
    pub dry_run: bool,
    /// Additional configuration options
    #[serde(default)]
    pub additional_options: HashMap<String, serde_json::Value>,
}

impl ReleaseState {
    /// Create a new release state
    pub fn new(release_version: semver::Version, config: ReleaseConfig) -> Self {
        let now = chrono::Utc::now();
        let release_id = format!("release-{}-{}", release_version, now.timestamp());

        Self {
            format_version: STATE_FORMAT_VERSION,
            save_version: 0,
            release_id,
            release_version,
            started_at: now,
            updated_at: now,
            current_phase: ReleasePhase::Validation,
            checkpoints: Vec::new(),
            github_state: None,
            errors: Vec::new(),
            config,
        }
    }

    /// Add a checkpoint to the release state
    pub fn add_checkpoint(
        &mut self,
        name: String,
        phase: ReleasePhase,
        data: Option<serde_json::Value>,
    ) {
        let checkpoint = ReleaseCheckpoint {
            name,
            phase,
            timestamp: chrono::Utc::now(),
            data,
        };

        self.checkpoints.push(checkpoint);
        self.updated_at = chrono::Utc::now();
    }

    /// Check if a specific phase has been completed
    pub fn has_completed(&self, phase: ReleasePhase) -> bool {
        self.checkpoints.iter().any(|cp| cp.phase == phase)
    }

    /// Set current phase
    pub fn set_phase(&mut self, phase: ReleasePhase) {
        self.current_phase = phase;
        self.updated_at = chrono::Utc::now();
    }

    /// Add an error to the release state
    pub fn add_error(
        &mut self,
        message: String,
        phase: ReleasePhase,
        recoverable: bool,
        context: Option<String>,
    ) {
        let error = ReleaseError {
            message,
            phase,
            timestamp: chrono::Utc::now(),
            recoverable,
            context,
        };

        self.errors.push(error);
        self.updated_at = chrono::Utc::now();
    }

    /// Update GitHub release state
    pub fn set_github_state(
        &mut self,
        owner: String,
        repo: String,
        result: Option<&crate::github::GitHubReleaseResult>,
    ) {
        if let Some(result) = result {
            self.github_state = Some(GitHubState {
                owner,
                repo,
                release_id: Some(result.release_id),
                html_url: Some(result.html_url.clone()),
                draft: result.draft,
                prerelease: result.prerelease,
                uploaded_artifacts: Vec::new(),
            });
        }
        self.updated_at = chrono::Utc::now();
    }

    /// Check if release is resumable
    pub fn is_resumable(&self) -> bool {
        !matches!(self.current_phase, ReleasePhase::Completed | ReleasePhase::Failed)
            && !self.has_critical_errors()
    }

    /// Check if release has critical errors
    pub fn has_critical_errors(&self) -> bool {
        self.errors.iter().any(|e| !e.recoverable)
    }

    /// Get progress percentage
    pub fn progress_percentage(&self) -> f64 {
        match self.current_phase {
            ReleasePhase::Validation => 10.0,
            ReleasePhase::GitHubRelease => 20.0,
            ReleasePhase::Building => 40.0,
            ReleasePhase::Bundling => 60.0,
            ReleasePhase::Uploading => 80.0,
            ReleasePhase::GitHubPublish => 90.0,
            ReleasePhase::Completed => 100.0,
            ReleasePhase::Failed => 0.0,
        }
    }

    /// Get elapsed time
    pub fn elapsed_time(&self) -> chrono::Duration {
        self.updated_at - self.started_at
    }

    /// Validate state consistency
    pub fn validate(&self) -> Result<()> {
        if self.format_version != STATE_FORMAT_VERSION {
            return Err(StateError::VersionMismatch {
                expected: STATE_FORMAT_VERSION.to_string(),
                found: self.format_version.to_string(),
            }
            .into());
        }
        Ok(())
    }

    /// Create a summary of the release state
    pub fn summary(&self) -> String {
        let elapsed = self.elapsed_time();
        let progress = self.progress_percentage();

        format!(
            "Release v{} ({:?}) - {:.1}% complete - {} elapsed",
            self.release_version,
            self.current_phase,
            progress,
            format_duration(elapsed)
        )
    }
}

fn format_duration(duration: chrono::Duration) -> String {
    let total_seconds = duration.num_seconds();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

impl std::fmt::Display for ReleasePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReleasePhase::Validation => write!(f, "Validation"),
            ReleasePhase::GitHubRelease => write!(f, "GitHub Release"),
            ReleasePhase::Building => write!(f, "Building"),
            ReleasePhase::Bundling => write!(f, "Bundling"),
            ReleasePhase::Uploading => write!(f, "Uploading"),
            ReleasePhase::GitHubPublish => write!(f, "GitHub Publish"),
            ReleasePhase::Completed => write!(f, "Completed"),
            ReleasePhase::Failed => write!(f, "Failed"),
        }
    }
}

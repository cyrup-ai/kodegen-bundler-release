//! Error types for release operations.

#![allow(dead_code)]

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for release operations
pub type Result<T> = std::result::Result<T, ReleaseError>;

/// Main error type for all release operations
#[derive(Error, Debug)]
pub enum ReleaseError {
    /// Workspace analysis errors
    #[error("Workspace error: {0}")]
    Workspace(#[from] WorkspaceError),

    /// State management errors
    #[error("State error: {0}")]
    State(#[from] StateError),

    /// CLI argument errors
    #[error("CLI error: {0}")]
    Cli(#[from] CliError),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML parsing errors
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    /// GitHub operation errors
    #[error("GitHub error: {0}")]
    GitHub(String),

    /// Generic errors from anyhow
    #[error("{0}")]
    Anyhow(#[from] anyhow::Error),
}

/// Workspace-specific errors
#[derive(Error, Debug)]
pub enum WorkspaceError {
    /// Workspace root not found
    #[error("Could not find workspace root. Please run from within a Cargo workspace.")]
    RootNotFound,

    /// Invalid workspace structure
    #[error("Invalid workspace structure: {reason}")]
    InvalidStructure {
        reason: String,
    },

    /// Package not found in workspace
    #[error("Package '{name}' not found in workspace")]
    PackageNotFound {
        name: String,
    },

    /// Missing Cargo.toml file
    #[error("Missing Cargo.toml file at {path}")]
    MissingCargoToml {
        path: PathBuf,
    },

    /// Invalid package configuration
    #[error("Invalid package configuration for '{package}': {reason}")]
    InvalidPackage {
        package: String,
        reason: String,
    },
}

/// State management errors
#[derive(Error, Debug)]
pub enum StateError {
    /// State file corrupted
    #[error("State file corrupted: {reason}")]
    Corrupted {
        reason: String,
    },

    /// State file not found
    #[error("State file not found. No release in progress.")]
    NotFound,

    /// State version mismatch
    #[error("State file version mismatch: expected {expected}, found {found}")]
    VersionMismatch {
        expected: String,
        found: String,
    },

    /// Concurrent modification detected
    #[error("Concurrent modification detected: state was modified by another process (expected save_version {expected}, found {found})")]
    ConcurrentModification {
        expected: u64,
        found: u64,
    },

    /// Failed to save state
    #[error("Failed to save state: {reason}")]
    SaveFailed {
        reason: String,
    },

    /// Failed to load state
    #[error("Failed to load state: {reason}")]
    LoadFailed {
        reason: String,
    },
}

/// CLI-specific errors
#[derive(Error, Debug)]
pub enum CliError {
    /// Invalid command line arguments
    #[error("Invalid arguments: {reason}")]
    InvalidArguments {
        reason: String,
    },

    /// Missing required argument
    #[error("Missing required argument: {argument}")]
    MissingArgument {
        argument: String,
    },

    /// Conflicting arguments
    #[error("Conflicting arguments: {arguments:?}")]
    ConflictingArguments {
        arguments: Vec<String>,
    },

    /// Command execution failed
    #[error("Command execution failed: {command} - {reason}")]
    ExecutionFailed {
        command: String,
        reason: String,
    },
}

impl ReleaseError {
    /// Get actionable recovery suggestions for this error
    pub fn recovery_suggestions(&self) -> Vec<String> {
        match self {
            ReleaseError::Workspace(WorkspaceError::RootNotFound) => vec![
                "Navigate to a directory containing a Cargo workspace".to_string(),
                "Ensure you have a Cargo.toml file with [workspace] section".to_string(),
            ],
            _ => vec!["Check the error message above for specific details".to_string()],
        }
    }

    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        !matches!(
            self,
            ReleaseError::Workspace(WorkspaceError::RootNotFound)
        )
    }
}

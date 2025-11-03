//! Artifact verification and discovery for Docker builds.
//!
//! Handles finding and verifying package artifacts created by Docker containers.

use crate::error::{CliError, ReleaseError};
use std::path::PathBuf;

/// Verifies that artifacts are complete and not corrupted.
///
/// Checks:
/// - File exists and is readable
/// - File size > 0 (not empty)
///
/// # Arguments
///
/// * `artifacts` - Paths to artifact files to verify
/// * `runtime_config` - For verbose output
pub fn verify_artifacts(
    artifacts: &[PathBuf],
    runtime_config: &crate::cli::RuntimeConfig,
) -> Result<(), ReleaseError> {
    for artifact in artifacts {
        // Check file exists and get metadata
        let metadata = std::fs::metadata(artifact).map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "verify artifact".to_string(),
                reason: format!("Cannot read artifact {}: {}", artifact.display(), e),
            })
        })?;

        // Check file is not empty
        if metadata.len() == 0 {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "verify artifact".to_string(),
                reason: format!(
                    "Artifact is empty (0 bytes): {}\n\
                     This indicates a failed or incomplete build.",
                    artifact.display()
                ),
            }));
        }

        // Success - log verification
        runtime_config.indent(&format!(
            "  ✓ Verified: {} ({} bytes)",
            artifact
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unknown>"),
            metadata.len()
        ));
    }

    Ok(())
}

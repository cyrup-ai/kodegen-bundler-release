//! Embedded .devcontainer resources for Docker-based cross-platform bundling.
//!
//! This module embeds the .devcontainer configuration files at compile time,
//! allowing the bundler to create Docker build environments without requiring
//! the source .devcontainer directory at runtime.

use crate::error::{CliError, ReleaseError, Result};
use std::fs;
use std::path::Path;

/// Embedded Dockerfile for multi-platform builds
/// 
/// This Dockerfile provides a unified build environment supporting:
/// - Linux packages (.deb, .rpm, AppImage)
/// - Windows packages (.msi via WiX/Wine, .exe via NSIS)
/// - All builds run in a Debian-based container with cross-platform tooling
const DOCKERFILE: &str = include_str!("../../../.devcontainer/Dockerfile");

/// Embedded README.md with setup instructions
const README: &str = include_str!("../../../.devcontainer/README.md");

/// Embedded devcontainer.json for VS Code Dev Containers
const DEVCONTAINER_JSON: &str = include_str!("../../../.devcontainer/devcontainer.json");

/// Copy embedded .devcontainer files to target directory
///
/// Creates a `.devcontainer/` subdirectory in the target path and writes
/// all embedded configuration files. This allows Docker image building to
/// work in temporary clones without requiring the original source directory.
///
/// # Arguments
///
/// * `target_dir` - Directory where .devcontainer/ should be created
///
/// # Returns
///
/// * `Ok(())` - All files written successfully
/// * `Err` - Failed to create directory or write files
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
/// use kodegen_bundler_release::cli::commands::copy_embedded_devcontainer;
///
/// let temp_clone = Path::new("/tmp/kodegen-release-12345");
/// copy_embedded_devcontainer(temp_clone)?;
/// // Now temp_clone/.devcontainer/{Dockerfile,README.md,devcontainer.json} exist
/// ```
pub fn copy_embedded_devcontainer(target_dir: &Path) -> Result<()> {
    let devcontainer_dir = target_dir.join(".devcontainer");
    
    // Create .devcontainer directory with standard permissions
    fs::create_dir_all(&devcontainer_dir).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "create_devcontainer_dir".to_string(),
            reason: format!(
                "Failed to create .devcontainer directory at {}: {}",
                devcontainer_dir.display(),
                e
            ),
        })
    })?;
    
    // Write Dockerfile (required for Docker image builds)
    fs::write(devcontainer_dir.join("Dockerfile"), DOCKERFILE).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "write_dockerfile".to_string(),
            reason: format!(
                "Failed to write Dockerfile to {}: {}",
                devcontainer_dir.join("Dockerfile").display(),
                e
            ),
        })
    })?;
    
    // Write README.md (documentation)
    fs::write(devcontainer_dir.join("README.md"), README).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "write_readme".to_string(),
            reason: format!(
                "Failed to write README.md to {}: {}",
                devcontainer_dir.join("README.md").display(),
                e
            ),
        })
    })?;
    
    // Write devcontainer.json (VS Code Dev Containers configuration)
    fs::write(
        devcontainer_dir.join("devcontainer.json"),
        DEVCONTAINER_JSON,
    )
    .map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "write_devcontainer_json".to_string(),
            reason: format!(
                "Failed to write devcontainer.json to {}: {}",
                devcontainer_dir.join("devcontainer.json").display(),
                e
            ),
        })
    })?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_copy_embedded_devcontainer() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let result = copy_embedded_devcontainer(temp_dir.path());
        
        assert!(result.is_ok(), "Failed to copy devcontainer: {:?}", result);
        
        // Verify directory exists
        let devcontainer_dir = temp_dir.path().join(".devcontainer");
        assert!(devcontainer_dir.exists());
        assert!(devcontainer_dir.is_dir());
        
        // Verify all files exist and are non-empty
        let dockerfile = devcontainer_dir.join("Dockerfile");
        assert!(dockerfile.exists());
        let dockerfile_contents = fs::read_to_string(&dockerfile).unwrap();
        assert!(!dockerfile_contents.is_empty());
        assert!(dockerfile_contents.contains("FROM rust:"));
        
        let readme = devcontainer_dir.join("README.md");
        assert!(readme.exists());
        let readme_contents = fs::read_to_string(&readme).unwrap();
        assert!(!readme_contents.is_empty());
        
        let devcontainer_json = devcontainer_dir.join("devcontainer.json");
        assert!(devcontainer_json.exists());
        let json_contents = fs::read_to_string(&devcontainer_json).unwrap();
        assert!(!json_contents.is_empty());
        assert!(json_contents.contains("Dockerfile"));
    }
}

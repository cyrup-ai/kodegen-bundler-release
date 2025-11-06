//! Version management for single-package releases.
//!
//! This module provides semantic version bumping and TOML editing for
//! single-repository (non-workspace) releases.

mod bumper;

pub use bumper::{VersionBump, VersionBumper};

use crate::error::{Result, VersionError};

/// Update version in a single Cargo.toml file (not workspace-aware).
///
/// This is a simplified version updater for single-repository releases.
/// Returns the parsed TOML content for in-memory verification without re-reading the file.
pub fn update_single_toml(cargo_toml_path: &std::path::Path, new_version: &str) -> Result<toml::Value> {
    let content = std::fs::read_to_string(cargo_toml_path)
        .map_err(|e| VersionError::TomlUpdateFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: format!("Failed to read file: {}", e),
        })?;
    
    let mut doc = content.parse::<toml_edit::DocumentMut>()
        .map_err(|e| VersionError::TomlUpdateFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: format!("Failed to parse TOML: {}", e),
        })?;
    
    // Update [package] version
    doc["package"]["version"] = toml_edit::value(new_version);
    
    let updated_content = doc.to_string();
    
    std::fs::write(cargo_toml_path, &updated_content)
        .map_err(|e| VersionError::TomlUpdateFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: format!("Failed to write file: {}", e),
        })?;
    
    // Parse the written content into toml::Value for verification
    // This avoids re-reading the file from disk
    let parsed_toml = toml::from_str(&updated_content)
        .map_err(|e| VersionError::TomlUpdateFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: format!("Failed to parse updated TOML: {}", e),
        })?;
    
    Ok(parsed_toml)
}

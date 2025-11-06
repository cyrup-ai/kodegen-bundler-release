//! Temporary metadata module for release package.
//!
//! This is a minimal replacement for the deleted metadata module.
//! TODO: This should be replaced with proper metadata handling in the future.

use crate::error::{ReleaseError, Result};
use std::path::Path;

/// Package metadata extracted from Cargo.toml
#[derive(Debug, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
}

/// Manifest with metadata and binary name
pub struct Manifest {
    pub metadata: PackageMetadata,
    pub binary_name: String,
}

/// Load manifest from Cargo.toml
///
/// This is a minimal implementation that extracts package name, version, and binary name.
pub fn load_manifest(cargo_toml_path: &Path) -> Result<Manifest> {
    let content = std::fs::read_to_string(cargo_toml_path)
        .map_err(ReleaseError::Io)?;

    // Parse package name
    let name = content
        .lines()
        .find(|line| line.trim().starts_with("name = "))
        .and_then(|line| line.split('=').nth(1))
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| ReleaseError::Cli(crate::error::CliError::InvalidArguments {
            reason: format!("Could not find package name in {}", cargo_toml_path.display()),
        }))?;

    // Parse version
    let version = content
        .lines()
        .find(|line| line.trim().starts_with("version = "))
        .and_then(|line| line.split('=').nth(1))
        .map(|s| s.trim().trim_matches('"').to_string())
        .ok_or_else(|| ReleaseError::Cli(crate::error::CliError::InvalidArguments {
            reason: format!("Could not find version in {}", cargo_toml_path.display()),
        }))?;

    // Binary name defaults to package name (simplified - doesn't handle [[bin]] sections)
    let binary_name = name.clone();

    Ok(Manifest {
        metadata: PackageMetadata {
            name: name.clone(),
            version,
        },
        binary_name,
    })
}

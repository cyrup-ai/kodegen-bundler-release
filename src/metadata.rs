//! Metadata and binary discovery from Cargo.toml

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
/// Properly handles [[bin]] sections in Cargo.toml for binary discovery.
/// Falls back to package name if no [[bin]] sections exist.
pub fn load_manifest(cargo_toml_path: &Path) -> Result<Manifest> {
    // Step 1: Read file once
    let content = std::fs::read_to_string(cargo_toml_path).map_err(|e| {
        ReleaseError::Cli(crate::error::CliError::ExecutionFailed {
            command: "read_cargo_toml".to_string(),
            reason: format!("Failed to read {}: {}", cargo_toml_path.display(), e),
        })
    })?;

    // Step 2: Parse TOML once
    let toml_value: toml::Value = toml::from_str(&content).map_err(|e| {
        ReleaseError::Cli(crate::error::CliError::ExecutionFailed {
            command: "parse_cargo_toml".to_string(),
            reason: format!("Failed to parse Cargo.toml: {}", e),
        })
    })?;

    let package = toml_value.get("package").ok_or_else(|| {
        ReleaseError::Cli(crate::error::CliError::InvalidArguments {
            reason: "No [package] section in Cargo.toml".to_string(),
        })
    })?;

    // Step 3: Extract package name
    let name = package
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ReleaseError::Cli(crate::error::CliError::InvalidArguments {
                reason: "Missing 'name' in [package]".to_string(),
            })
        })?
        .to_string();

    // Step 4: Extract version
    let version = package
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ReleaseError::Cli(crate::error::CliError::InvalidArguments {
                reason: "Missing 'version' in [package]".to_string(),
            })
        })?
        .to_string();

    // Step 5: Discover binary name from [[bin]] sections or fallback to package name
    let binary_name = toml_value
        .get("bin")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("name"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| Some(name.clone()))
        .ok_or_else(|| {
            ReleaseError::Cli(crate::error::CliError::InvalidArguments {
                reason: "No binary found in Cargo.toml".to_string(),
            })
        })?;

    Ok(Manifest {
        metadata: PackageMetadata {
            name,
            version,
        },
        binary_name,
    })
}

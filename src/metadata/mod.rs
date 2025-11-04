//! Metadata and binary discovery from single Cargo.toml

use crate::error::{CliError, ReleaseError, Result};
use std::path::Path;

/// Package metadata extracted from Cargo.toml
pub struct PackageMetadata {
    pub name: String,
    pub description: String,
    pub version: String,
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
}

/// Extract metadata from Cargo.toml [package] section
pub fn extract_metadata(cargo_toml_path: &Path) -> Result<PackageMetadata> {
    let manifest = std::fs::read_to_string(cargo_toml_path).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "read_cargo_toml".to_string(),
            reason: format!("Failed to read {}: {}", cargo_toml_path.display(), e),
        })
    })?;

    let toml_value: toml::Value = toml::from_str(&manifest).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "parse_cargo_toml".to_string(),
            reason: format!("Failed to parse Cargo.toml: {}", e),
        })
    })?;

    let package = toml_value.get("package").ok_or_else(|| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason: "No [package] section in Cargo.toml".to_string(),
        })
    })?;

    // REUSE PackageConfig pattern from workspace/analyzer.rs:76-98
    Ok(PackageMetadata {
        name: package
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ReleaseError::Cli(CliError::InvalidArguments {
                    reason: "Missing 'name' in [package]".to_string(),
                })
            })?
            .to_string(),

        description: package
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("Rust application")
            .to_string(),

        version: package
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ReleaseError::Cli(CliError::InvalidArguments {
                    reason: "Missing 'version' in [package]".to_string(),
                })
            })?
            .to_string(),

        authors: package
            .get("authors")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),

        license: package
            .get("license")
            .and_then(|v| v.as_str())
            .map(String::from),

        homepage: package
            .get("homepage")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// Auto-discover binary name from Cargo.toml
pub fn discover_binary(cargo_toml_path: &Path) -> Result<String> {
    let manifest = std::fs::read_to_string(cargo_toml_path).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "read_cargo_toml".to_string(),
            reason: format!("Failed to read {}: {}", cargo_toml_path.display(), e),
        })
    })?;

    let toml_value: toml::Value = toml::from_str(&manifest).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "parse_cargo_toml".to_string(),
            reason: format!("Failed to parse Cargo.toml: {}", e),
        })
    })?;

    // REUSE binary discovery pattern from helpers.rs:25-34
    // Try [[bin]] section first
    if let Some(bin_array) = toml_value.get("bin").and_then(|v| v.as_array()) {
        if let Some(first) = bin_array.first() {
            if let Some(name) = first.get("name").and_then(|v| v.as_str()) {
                return Ok(name.to_string());
            }
        }
    }

    // Fallback to package name
    if let Some(name) = toml_value
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
    {
        return Ok(name.to_string());
    }

    Err(ReleaseError::Cli(CliError::InvalidArguments {
        reason: "No binary found in Cargo.toml".to_string(),
    }))
}

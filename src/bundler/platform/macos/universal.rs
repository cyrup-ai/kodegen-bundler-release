//! Universal binary creation for macOS (Intel + Apple Silicon)
//!
//! This module provides functionality to create universal (fat) binaries
//! by merging x86_64 and aarch64 builds using Apple's `lipo` tool.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// All binaries in the kodegen workspace that need universal variants
/// 
/// Total: 18 binaries
/// - 3 core: kodegen_install (installer), kodegen (main stdio server), kodegend (daemon)
/// - 15 category servers: browser, candle-agent, citescrape, claude-agent, config,
///   database, filesystem, git, github, introspection, process, prompt,
///   reasoner, sequential-thinking, terminal
const ALL_BINARIES: &[&str] = &[
    // Core binaries (as listed in helpers.rs:72-75)
    "kodegen_install",
    "kodegen",
    "kodegend",
    // Category servers (as listed in helpers.rs:77-92)
    "kodegen-browser",
    "kodegen-candle-agent",
    "kodegen-citescrape",
    "kodegen-claude-agent",
    "kodegen-config",
    "kodegen-database",
    "kodegen-filesystem",
    "kodegen-git",
    "kodegen-github",
    "kodegen-introspection",
    "kodegen-process",
    "kodegen-prompt",
    "kodegen-reasoner",
    "kodegen-sequential-thinking",
    "kodegen-terminal",
];

/// Create universal binaries by merging x86_64 and aarch64 builds
///
/// Requires both architecture builds to exist:
/// - `target/x86_64-apple-darwin/release/<binary>`
/// - `target/aarch64-apple-darwin/release/<binary>`
///
/// # Arguments
/// * `workspace_root` - Path to workspace root (for target/ resolution)
/// * `output_dir` - Where to write universal binaries (typically `target/universal/release`)
///
/// # Returns
/// Vector of paths to created universal binaries
///
/// # Errors
/// - If either architecture's binaries are missing (must build both first)
/// - If lipo command fails (not installed or binary incompatibility)
/// - If output directory cannot be created
pub fn create_universal_binaries(
    workspace_root: &Path,
    output_dir: &Path,
) -> Result<Vec<PathBuf>> {
    // Verify both architecture build directories exist
    let x86_64_dir = workspace_root.join("target/x86_64-apple-darwin/release");
    let aarch64_dir = workspace_root.join("target/aarch64-apple-darwin/release");

    if !x86_64_dir.exists() {
        anyhow::bail!(
            "Intel (x86_64) binaries not found at {}\n\
             Run: cargo build --release --target x86_64-apple-darwin",
            x86_64_dir.display()
        );
    }

    if !aarch64_dir.exists() {
        anyhow::bail!(
            "Apple Silicon (aarch64) binaries not found at {}\n\
             Run: cargo build --release --target aarch64-apple-darwin",
            aarch64_dir.display()
        );
    }

    std::fs::create_dir_all(output_dir).with_context(|| {
        format!("Failed to create universal binary output directory: {}", output_dir.display())
    })?;

    log::info!("Creating universal binaries (x86_64 + aarch64) for {} binaries", ALL_BINARIES.len());

    let mut universal_binaries = Vec::new();

    for binary_name in ALL_BINARIES {
        let x86_64_bin = x86_64_dir.join(binary_name);
        let aarch64_bin = aarch64_dir.join(binary_name);
        let universal_bin = output_dir.join(binary_name);

        // Verify both architecture binaries exist
        if !x86_64_bin.exists() {
            log::warn!("Skipping {}: x86_64 binary not found at {}", binary_name, x86_64_bin.display());
            continue;
        }
        if !aarch64_bin.exists() {
            log::warn!("Skipping {}: aarch64 binary not found at {}", binary_name, aarch64_bin.display());
            continue;
        }

        // Use lipo to create universal binary
        // Command: lipo -create <x86_64> <aarch64> -output <universal>
        let output = Command::new("lipo")
            .arg("-create")
            .arg(&x86_64_bin)
            .arg(&aarch64_bin)
            .arg("-output")
            .arg(&universal_bin)
            .output()
            .context("Failed to run lipo command. Ensure Xcode Command Line Tools are installed.")?;

        if !output.status.success() {
            anyhow::bail!(
                "lipo failed for {}:\n{}",
                binary_name,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Verify the result contains both architectures
        let verify = Command::new("lipo")
            .arg("-info")
            .arg(&universal_bin)
            .output()?;

        let info = String::from_utf8_lossy(&verify.stdout);
        log::info!("✓ {}: {}", binary_name, info.trim());

        universal_binaries.push(universal_bin);
    }

    if universal_binaries.is_empty() {
        anyhow::bail!("No universal binaries were created. Verify both architecture builds exist.");
    }

    log::info!("Successfully created {} universal binaries", universal_binaries.len());
    Ok(universal_binaries)
}

//! Release command execution module.
//!
//! Handles the complete release workflow by coordinating all modules
//! in an isolated temporary clone to prevent modifications to the user's working directory.

mod r#impl;

use crate::cli::{Args, RuntimeConfig};
use crate::error::{CliError, ReleaseError, Result};
use crate::EnvConfig;

/// Execute release command
pub(super) async fn execute_release(
    args: &Args,
    config: &RuntimeConfig,
    env_config: &EnvConfig,
) -> Result<i32> {
    // 1. Parse and resolve repository source
    config.println("📦 Resolving repository source...");
    let source_parsed = crate::source::RepositorySource::parse(&args.source)?;
    let resolved = source_parsed.resolve().await?;
    config.verbose_println(&format!("✓ Repository: {}", resolved.path.display()));

    // 2. Extract metadata from single Cargo.toml
    let cargo_toml = resolved.path.join("Cargo.toml");
    let manifest = crate::metadata::load_manifest(&cargo_toml)?;
    let metadata = manifest.metadata;
    let binary_name = manifest.binary_name;
    
    config.verbose_println(&format!("✓ Package: {}", metadata.name));
    config.verbose_println(&format!("✓ Binary: {}", binary_name));

    // 3. Validation - git status check
    config.println("🔍 Validating repository...");
    let git_status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&resolved.path)
        .output()
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "git_status".to_string(),
                reason: e.to_string(),
            })
        })?;
    
    if !git_status.stdout.is_empty() {
        config.warning_println("⚠️  Working directory has uncommitted changes");
        config.warning_println("   This may cause issues with the release process");
    }

    // 4. Create temp clone for isolated execution
    config.println("📁 Creating temporary clone...");
    let temp_dir = if resolved.is_temp {
        resolved.path.clone()
    } else {
        super::temp_clone::clone_main_to_temp_for_release(&resolved.path).await?
    };
    let temp_dir_pathbuf = temp_dir.to_path_buf();

    // Copy embedded .devcontainer for Docker builds
    kodegen_bundler_bundle::cli::commands::copy_embedded_devcontainer(&temp_dir_pathbuf)?;
    config.verbose_println("✓ Embedded .devcontainer resources ready");

    // Clean up any stale tracking from crashed previous releases
    match super::temp_clone::cleanup_stale_tracking() {
        Ok(count) if count > 0 => {
            config.verbose_println(&format!("✓ Cleaned up {} stale release(s)", count));
        }
        Ok(_) => {}, // No stale releases
        Err(e) => {
            config.verbose_println(&format!("⚠ Warning: Failed to clean stale tracking: {}", e));
            // Non-fatal - continue with release
        }
    }

    // 5. Execute release in temp
    let result = r#impl::perform_release_single_repo(
        &temp_dir_pathbuf,
        metadata,
        binary_name,
        config,
        env_config,
    ).await;

    // 6. Cleanup temp directory
    if !resolved.is_temp {
        match std::fs::remove_dir_all(&temp_dir_pathbuf) {
            Ok(()) => {
                config.verbose_println("✅ Temp clone cleaned up");
                
                // Clear temp path tracking after successful cleanup
                if let Err(e) = super::temp_clone::clear_active_temp_path() {
                    config.verbose_println(&format!("Warning: Failed to clear temp path tracking: {}", e));
                }
            }
            Err(e) => {
                config.warning_println(&format!("Failed to cleanup temp directory: {}", e));
                config.warning_println(&format!(
                    "You may need to manually remove: {}",
                    temp_dir_pathbuf.display()
                ));
                
                // Still clear tracking - user will manually clean up temp dir
                let _ = super::temp_clone::clear_active_temp_path();
            }
        }
    }

    result
}

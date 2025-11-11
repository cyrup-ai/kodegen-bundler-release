//!
//! This module contains the primary `perform_release_single_repo` function that
//! coordinates all release phases with state management and error recovery.

use crate::cli::RuntimeConfig;
use crate::error::{ReleaseError, Result};
use crate::git::{GitConfig, GitManager};
use crate::state::{LoadStateResult, ReleaseState};
use crate::version::VersionBump;
use crate::EnvConfig;

use super::super::super::helpers::detect_github_repo;
use super::context::ReleasePhaseContext;
use super::phases::execute_phases_with_retry;
use super::retry::retry_with_backoff;

/// Perform release for a single repository (not a workspace).
///
/// This is the simplified release flow for individual packages.
/// Replaces the workspace-aware perform_release_impl().
pub async fn perform_release_single_repo(
    temp_dir: &std::path::Path,
    metadata: crate::metadata::PackageMetadata,
    binary_name: String,
    config: &RuntimeConfig,
    env_config: &EnvConfig,
) -> Result<i32> {
    config.println("🚀 Starting release in isolated environment").expect("Failed to write to stdout");
    
    // ===== LOAD OR CREATE RELEASE STATE =====
    let mut release_state = load_or_create_release_state(temp_dir, &metadata, config).await?;
    
    // Extract version information for use in subsequent code
    let new_version = release_state.target_version.clone();
    let version_bump = release_state.version_bump.clone();
    
    // Parse current version from metadata for display
    let current_version = semver::Version::parse(&metadata.version)
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
            version: metadata.version.clone(),
            reason: e.to_string(),
        }))?;
    
    // ===== PHASE 0.5: VALIDATE TEMP CLONE IS CLEAN =====
    // Create git_manager early to validate temp clone and for cleanup operations
    let git_config = GitConfig {
        default_remote: "origin".to_string(),
        annotated_tags: true,
        auto_push_tags: true,  // Always push tags
        ..Default::default()
    };
    let git_manager = GitManager::with_config(temp_dir, git_config).await?;

    // ===== PHASE 1: VERSION BUMP =====
    config.println("🔢 Bumping version...").expect("Failed to write to stdout");

    let cargo_toml_path = temp_dir.join("Cargo.toml");

    config.success_println(&format!("✓ v{} → v{} ({})", current_version, new_version, version_bump)).expect("Failed to write to stdout");
    
    // Use default retry and timeout configs
    use crate::cli::retry_config::{RetryConfig, CargoTimeoutConfig};
    let retry_config = RetryConfig::default();
    let timeout_config = CargoTimeoutConfig::default();
    
    // Update Cargo.toml with new version and get parsed content for verification
    let updated_toml_value = crate::version::update_single_toml(&cargo_toml_path, &new_version.to_string())?;
    
    // Verify the version was correctly written using the parsed content (faster, no disk read)
    verify_version_in_parsed_toml(&updated_toml_value, &new_version)?;
    
    // Update Cargo.lock with new version
    config.verbose_println("   Updating Cargo.lock...").expect("Failed to write to stdout");
    
    let update_lock_result = retry_with_backoff(
        || async {
            tokio::process::Command::new("cargo")
                .arg("update")
                .arg("--workspace")
                .arg("--verbose")
                .current_dir(temp_dir)
                .output()
                .await
                .map_err(|e| ReleaseError::Cli(crate::error::CliError::ExecutionFailed {
                    command: "cargo update".to_string(),
                    reason: e.to_string(),
                }))
        },
        retry_config.git_operations,  // Use git_operations for cargo operations
        "Cargo lock update",
        config,
        Some(std::time::Duration::from_secs(timeout_config.update_timeout_secs)),
    ).await?;
    
    if !update_lock_result.status.success() {
        return Err(ReleaseError::Cli(crate::error::CliError::ExecutionFailed {
            command: "cargo update".to_string(),
            reason: String::from_utf8_lossy(&update_lock_result.stderr).to_string(),
        }));
    }

    config.success_println("✓ Updated and verified Cargo.toml and Cargo.lock").expect("Failed to write to stdout");

    // Save state after version bump
    release_state.set_phase(crate::state::ReleasePhase::VersionUpdate);
    crate::state::save_release_state(temp_dir, &mut release_state).await?;
    config.verbose_println("ℹ️  Saved progress checkpoint (Version bumped)").expect("Failed to write to stdout");

    // ===== PHASE 2: DETECT AND RESOLVE CONFLICTS =====
    config.println("🔍 Checking for conflicting artifacts...").expect("Failed to write to stdout");
    
    // Check if tag exists
    let tag_exists = git_manager.version_tag_exists(&new_version).await?;
    
    if tag_exists {
        config.warning_println(&format!("⚠️  Tag v{} already exists - cleaning up...", new_version)).expect("Failed to write to stdout");
        git_manager.cleanup_existing_tag(&new_version).await?;
        config.success_println("✓ Cleaned up existing tag").expect("Failed to write to stdout");
    }

    // Check if release branch exists
    let branch_exists = git_manager.remote_release_branch_exists(&new_version).await?;

    if branch_exists {
        config.warning_println(&format!("⚠️  Branch release-v{} already exists - cleaning up...", new_version)).expect("Failed to write to stdout");
        git_manager.cleanup_existing_branch(&new_version).await?;
        config.success_println("✓ Cleaned up existing branch").expect("Failed to write to stdout");
    }

    config.success_println("✓ All conflicts resolved - ready to release").expect("Failed to write to stdout");

    // ===== PHASE 3+: GITHUB SETUP AND REMAINING PHASES =====
    config.println("🔍 Verifying GitHub API access...").expect("Failed to write to stdout");
    
    // Auto-detect GitHub repository
    let (github_owner, github_repo_name) = detect_github_repo(&git_manager).await?;

    config.verbose_println(&format!(
        "   Repository: {}/{}",
        &github_owner, &github_repo_name
    )).expect("Failed to write to stdout");
    
    // Initialize GitHub manager
    let github_config = crate::github::GitHubReleaseConfig {
        owner: github_owner.clone(),
        repo: github_repo_name.clone(),
        draft: false,
        prerelease_for_zero_versions: true,
        notes: None,
        token: None,  // Will be read from env_config in new()
    };
    
    let github_manager = crate::github::GitHubReleaseManager::new(github_config, env_config)?;
    config.success_println("✓ GitHub API authenticated").expect("Failed to write to stdout");
    
    // ===== BUILD CONTEXT FOR PHASE EXECUTION =====
    let ctx = ReleasePhaseContext {
        release_clone_path: temp_dir,
        binary_name: &binary_name,
        new_version: &new_version,
        config,
        git_manager: &git_manager,
        github_manager: &github_manager,
        github_owner: &github_owner,
        github_repo_name: &github_repo_name,
    };
    
    // ===== EXECUTE REMAINING PHASES WITH RETRY =====
    execute_phases_with_retry(&ctx, &mut release_state, env_config).await?;
    
    // ===== CLEANUP RELEASE STATE ON SUCCESS =====
    config.success_println("🎉 Release complete!").expect("Failed to write to stdout");
    config.success_println(&format!("   Package: {}", metadata.name)).expect("Failed to write to stdout");
    config.success_println(&format!("   Version: v{}", new_version)).expect("Failed to write to stdout");

    // Cleanup release state file after successful release
    match crate::state::cleanup_release_state(temp_dir) {
        Ok(()) => {
            config.verbose_println("✓ Release state cleaned up").expect("Failed to write to stdout");
        }
        Err(e) => {
            config.verbose_println(&format!("Warning: Failed to cleanup release state: {}", e)).expect("Failed to write to stdout");
            config.verbose_println("This is non-fatal - the release completed successfully").expect("Failed to write to stdout");
        }
    }
    
    Ok(0)
}

/// Load existing release state or create a new one
async fn load_or_create_release_state(
    temp_dir: &std::path::Path,
    metadata: &crate::metadata::PackageMetadata,
    config: &RuntimeConfig,
) -> Result<ReleaseState> {
    if crate::state::has_active_release(temp_dir) {
        config.println("📂 Found existing release state - resuming...").expect("Failed to write to stdout");

        match crate::state::load_release_state(temp_dir).await {
            Ok(LoadStateResult { state, recovered_from_backup, warnings }) => {
                if recovered_from_backup {
                    config.warning_println("⚠️  State recovered from backup").expect("Failed to write to stdout");
                }
                for warning in &warnings {
                    config.warning_println(&format!("⚠️  {}", warning)).expect("Failed to write to stdout");
                }
                
                // Validate state version matches what we're trying to release
                let current_version = semver::Version::parse(&metadata.version)
                    .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
                        version: metadata.version.clone(),
                        reason: e.to_string(),
                    }))?;
                
                // Always patch bump
                let version_bump = VersionBump::Patch;
                
                let bumper = crate::version::VersionBumper::from_version(current_version.clone());
                let expected_version = bumper.bump(version_bump.clone())?;
                
                if state.target_version != expected_version {
                    config.warning_println(&format!(
                        "⚠️  State version mismatch: expected v{}, found v{}",
                        expected_version,
                        state.target_version
                    )).expect("Failed to write to stdout");
                    config.warning_println("   Starting fresh release...").expect("Failed to write to stdout");

                    // Clean up stale state file before creating new state
                    if let Err(e) = crate::state::cleanup_release_state(temp_dir) {
                        config.warning_println(&format!("⚠️  Failed to cleanup stale state: {}", e)).expect("Failed to write to stdout");
                    }

                    // Create new state
                    create_new_release_state(metadata)
                } else {
                    config.success_println(&format!("✓ Resuming release v{}", state.target_version)).expect("Failed to write to stdout");
                    config.indent(&format!("   Current phase: {:?}", state.current_phase)).expect("Failed to write to stdout");
                    config.indent(&format!("   Checkpoints: {}", state.checkpoints.len())).expect("Failed to write to stdout");
                    Ok(state)
                }
            }
            Err(e) => {
                config.warning_println(&format!("⚠️  Failed to load state: {}", e)).expect("Failed to write to stdout");
                config.warning_println("   Starting fresh release...").expect("Failed to write to stdout");

                // Clean up corrupted state file before creating new state
                if let Err(e) = crate::state::cleanup_release_state(temp_dir) {
                    config.warning_println(&format!("⚠️  Failed to cleanup corrupted state: {}", e)).expect("Failed to write to stdout");
                }
                
                create_new_release_state(metadata)
            }
        }
    } else {
        // No existing state - start fresh
        create_new_release_state(metadata)
    }
}

/// Create a new release state from metadata
fn create_new_release_state(
    metadata: &crate::metadata::PackageMetadata,
) -> Result<ReleaseState> {
    let current_version = semver::Version::parse(&metadata.version)
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
            version: metadata.version.clone(),
            reason: e.to_string(),
        }))?;
    
    // Always patch bump
    let version_bump = VersionBump::Patch;
    
    let bumper = crate::version::VersionBumper::from_version(current_version.clone());
    let new_version = bumper.bump(version_bump.clone())?;
    
    Ok(ReleaseState::new(
        new_version,
        version_bump,
        crate::state::ReleaseConfig::default(),
    ))
}

/// Verify that the version was correctly written to Cargo.toml using parsed TOML
///
/// This is more efficient than reading the file from disk again.
fn verify_version_in_parsed_toml(
    toml_value: &toml::Value,
    expected_version: &semver::Version,
) -> Result<()> {
    let version_str = toml_value
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ReleaseError::Version(crate::error::VersionError::InvalidVersion {
                version: "unknown".to_string(),
                reason: "Could not find package.version in updated Cargo.toml".to_string(),
            })
        })?;
    
    let parsed_version = semver::Version::parse(version_str).map_err(|e| {
        ReleaseError::Version(crate::error::VersionError::InvalidVersion {
            version: version_str.to_string(),
            reason: e.to_string(),
        })
    })?;
    
    if &parsed_version != expected_version {
        return Err(ReleaseError::Version(
            crate::error::VersionError::InvalidVersion {
                version: parsed_version.to_string(),
                reason: format!("Expected version {} but found {}", expected_version, parsed_version),
            },
        ));
    }
    
    Ok(())
}

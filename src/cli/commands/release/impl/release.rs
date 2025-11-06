//! Main release orchestration logic for single repositories.
//!
//! This module contains the primary `perform_release_single_repo` function that
//! coordinates all release phases with state management and error recovery.

use crate::cli::RuntimeConfig;
use crate::error::{GitError, ReleaseError, Result};
use crate::git::{GitConfig, GitManager};
use crate::state::{has_active_release, load_release_state, LoadStateResult, ReleaseState};

use super::super::super::helpers::{detect_github_repo, parse_github_repo_string};
use super::super::ReleaseOptions;
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
    options: &ReleaseOptions,
) -> Result<i32> {
    config.println("🚀 Starting release in isolated environment");
    
    // ===== LOAD OR CREATE RELEASE STATE =====
    let mut release_state = load_or_create_release_state(&metadata, options, config).await?;
    
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
        auto_push_tags: !options.no_push,
        ..Default::default()
    };
    let git_manager = GitManager::with_config(temp_dir, git_config).await?;
    
    // Validate working directory is clean before making any modifications
    if !git_manager.is_clean().await? {
        return Err(ReleaseError::Git(GitError::DirtyWorkingDirectory));
    }
    
    // ===== PHASE 1: VERSION BUMP =====
    config.println("🔢 Bumping version...");
    
    let cargo_toml_path = temp_dir.join("Cargo.toml");
    
    config.success_println(&format!("✓ v{} → v{} ({})", current_version, new_version, version_bump));
    
    // Update Cargo.toml with new version
    crate::version::update_single_toml(&cargo_toml_path, &new_version.to_string())?;

    // VERIFY: Read back and confirm version matches
    verify_version_update(&cargo_toml_path, &new_version)?;

    // Update Cargo.lock to match new version
    update_cargo_lock(temp_dir, &new_version).await?;

    config.success_println("✓ Updated and verified Cargo.toml and Cargo.lock");
    
    // ===== PHASE 1.5: AUTOMATIC CLEANUP OF CONFLICTS =====
    config.println("🔍 Checking for conflicting artifacts...");
    
    // Check and cleanup existing tag
    if git_manager.version_tag_exists(&new_version).await? {
        config.println(&format!("⚠️  Tag v{} already exists - cleaning up...", new_version));
        git_manager.cleanup_existing_tag(&new_version).await?;
        config.success_println("✓ Cleaned up existing tag");
    }
    
    // Check and cleanup existing branch (local or remote)
    let has_local_branch = git_manager.release_branch_exists(&new_version).await?;
    let has_remote_branch = git_manager.remote_release_branch_exists(&new_version).await?;
    
    if has_local_branch || has_remote_branch {
        config.println(&format!("⚠️  Branch v{} already exists - cleaning up...", new_version));
        git_manager.cleanup_existing_branch(&new_version).await?;
        config.success_println("✓ Cleaned up existing branch");
    }
    
    // Detect GitHub repo early for cleanup
    let (owner, repo) = if let Some(repo_str) = &options.github_repo {
        parse_github_repo_string(repo_str)?
    } else {
        detect_github_repo(&git_manager).await?
    };
    
    // Create github_manager early for cleanup
    let github_config = crate::github::GitHubReleaseConfig {
        owner: owner.clone(),
        repo: repo.clone(),
        draft: true,
        prerelease_for_zero_versions: true,
        notes: None,
        token: None, // From GH_TOKEN or GITHUB_TOKEN environment variable
    };
    let github_manager = crate::github::GitHubReleaseManager::new(github_config)?;
    
    // Check and cleanup existing GitHub release
    if github_manager.release_exists(&new_version).await? {
        config.println(&format!("⚠️  GitHub release v{} already exists - cleaning up...", new_version));
        github_manager.cleanup_existing_release(&new_version).await?;
        config.success_println("✓ Cleaned up existing GitHub release");
    }
    
    config.success_println("✓ All conflicts resolved - ready to release");
    
    // ===== EXECUTE PHASES 2-8 WITH RETRY AND SELECTIVE CLEANUP =====
    // Create context for phase execution
    let phase_ctx = ReleasePhaseContext {
        release_clone_path: temp_dir,
        metadata: &metadata,
        binary_name: &binary_name,
        new_version: &new_version,
        config,
        options,
        git_manager: &git_manager,
        github_manager: &github_manager,
        owner: &owner,
        repo: &repo,
    };
    
    // Execute phases with context
    let result = execute_phases_with_retry(&phase_ctx, &mut release_state).await;
    
    match result {
        Ok(()) => {
            // SUCCESS: Clear state and return
            git_manager.clear_release_state();
            
            // Delete state file on success
            if let Err(e) = crate::state::cleanup_release_state() {
                config.warning_println(&format!("⚠️  Failed to cleanup state file: {}", e));
            } else {
                config.verbose_println("ℹ️  Cleaned up state file (release complete)");
            }
            
            Ok(0)
        }
        Err(e) if !e.is_recoverable() => {
            // UNRECOVERABLE ERROR: Cleanup required
            handle_unrecoverable_error(e, &release_state, &git_manager, &github_manager, &owner, &repo, config).await
        }
        Err(e) => {
            // RECOVERABLE ERROR (retries exhausted) - No cleanup needed
            // The error is transient (network, timeout, etc.) - retrying might work next time
            // Don't cleanup git/GitHub artifacts as they're valid, just incomplete
            config.println("");
            config.error_println(&format!("❌ Release failed after retries: {}", e));
            config.println("");
            Err(e)
        }
    }
}

/// Load existing release state or create a new one
async fn load_or_create_release_state(
    metadata: &crate::metadata::PackageMetadata,
    options: &ReleaseOptions,
    config: &RuntimeConfig,
) -> Result<ReleaseState> {
    if has_active_release() {
        config.println("📂 Found existing release state - resuming...");
        
        match load_release_state().await {
            Ok(LoadStateResult { state, recovered_from_backup, warnings }) => {
                if recovered_from_backup {
                    config.warning_println("⚠️  State recovered from backup");
                }
                for warning in &warnings {
                    config.warning_println(&format!("⚠️  {}", warning));
                }
                
                // Validate state version matches what we're trying to release
                let current_version = semver::Version::parse(&metadata.version)
                    .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
                        version: metadata.version.clone(),
                        reason: e.to_string(),
                    }))?;
                
                let version_bump = crate::version::VersionBump::try_from(options.bump_type.clone())
                    .map_err(|e| ReleaseError::Cli(crate::error::CliError::InvalidArguments { reason: e }))?;
                
                let bumper = crate::version::VersionBumper::from_version(current_version.clone());
                let expected_version = bumper.bump(version_bump.clone())?;
                
                if state.target_version != expected_version {
                    config.warning_println(&format!(
                        "⚠️  State version mismatch: expected v{}, found v{}",
                        expected_version,
                        state.target_version
                    ));
                    config.warning_println("   Starting fresh release...");
                    
                    // Create new state
                    create_new_release_state(metadata, options)
                } else {
                    config.success_println(&format!("✓ Resuming release v{}", state.target_version));
                    config.indent(&format!("   Current phase: {:?}", state.current_phase));
                    config.indent(&format!("   Checkpoints: {}", state.checkpoints.len()));
                    Ok(state)
                }
            }
            Err(e) => {
                config.warning_println(&format!("⚠️  Failed to load state: {}", e));
                config.warning_println("   Starting fresh release...");
                create_new_release_state(metadata, options)
            }
        }
    } else {
        // No existing state - start fresh
        create_new_release_state(metadata, options)
    }
}

/// Create a new release state from metadata and options
fn create_new_release_state(
    metadata: &crate::metadata::PackageMetadata,
    options: &ReleaseOptions,
) -> Result<ReleaseState> {
    let current_version = semver::Version::parse(&metadata.version)
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
            version: metadata.version.clone(),
            reason: e.to_string(),
        }))?;
    
    let version_bump = crate::version::VersionBump::try_from(options.bump_type.clone())
        .map_err(|e| ReleaseError::Cli(crate::error::CliError::InvalidArguments { reason: e }))?;
    
    let bumper = crate::version::VersionBumper::from_version(current_version.clone());
    let new_version = bumper.bump(version_bump.clone())?;
    
    Ok(ReleaseState::new(
        new_version,
        version_bump,
        crate::state::ReleaseConfig::default(),
    ))
}

/// Verify that the version was correctly written to Cargo.toml
fn verify_version_update(cargo_toml_path: &std::path::Path, new_version: &semver::Version) -> Result<()> {
    let verification_content = std::fs::read_to_string(cargo_toml_path)
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::VerificationFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: format!("Cannot read back file after write: {}", e),
        }))?;

    let verification_parsed: toml::Value = toml::from_str(&verification_content)
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::VerificationFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: format!("TOML is invalid after update: {}", e),
        }))?;

    let written_version = verification_parsed
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| ReleaseError::Version(crate::error::VersionError::VerificationFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: "Version field missing after update".to_string(),
        }))?;

    if written_version != new_version.to_string() {
        return Err(ReleaseError::Version(crate::error::VersionError::VerificationFailed {
            path: cargo_toml_path.to_path_buf(),
            reason: format!(
                "Version mismatch: expected {}, found {}",
                new_version, written_version
            ),
        }));
    }

    Ok(())
}

/// Update Cargo.lock to match the new version
async fn update_cargo_lock(temp_dir: &std::path::Path, new_version: &semver::Version) -> Result<()> {
    use tokio::process::Command;
    use std::process::Stdio;

    let update_output = Command::new("cargo")
        .arg("update")
        .arg("--workspace")
        .current_dir(temp_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::CargoUpdateFailed {
            reason: format!("Failed to run cargo update: {}", e),
        }))?;

    if !update_output.status.success() {
        let stderr = String::from_utf8_lossy(&update_output.stderr);
        return Err(ReleaseError::Version(crate::error::VersionError::CargoUpdateFailed {
            reason: format!("cargo update failed:\n{}", stderr),
        }));
    }

    // Verify Cargo.lock was updated
    let lock_path = temp_dir.join("Cargo.lock");
    if lock_path.exists() {
        let lock_content = std::fs::read_to_string(&lock_path)
            .map_err(|e| ReleaseError::Version(crate::error::VersionError::VerificationFailed {
                path: lock_path.clone(),
                reason: format!("Cannot read Cargo.lock: {}", e),
            }))?;
        
        if !lock_content.contains(&new_version.to_string()) {
            return Err(ReleaseError::Version(crate::error::VersionError::LockFileMismatch {
                expected_version: new_version.to_string(),
            }));
        }
    }

    Ok(())
}

/// Handle unrecoverable errors with cleanup
async fn handle_unrecoverable_error(
    e: ReleaseError,
    release_state: &ReleaseState,
    git_manager: &GitManager,
    github_manager: &crate::github::GitHubReleaseManager,
    owner: &str,
    repo: &str,
    config: &RuntimeConfig,
) -> Result<i32> {
    config.println("");
    config.error_println(&format!("❌ Release failed with unrecoverable error: {}", e));
    config.println("");
    config.println("🧹 Rolling back changes...");
    
    let mut cleanup_warnings = Vec::new();
    
    // 1. Delete GitHub release if created (Phase 3+)
    if let Some(github_state) = &release_state.github_state
        && let Some(release_id) = github_state.release_id
    {
        config.indent("🗑️  Deleting GitHub draft release...");
        
        // Retry GitHub release deletion with exponential backoff
        let delete_result = retry_with_backoff(
            || github_manager.delete_release(release_id),
            config.retry_config.cleanup_operations,
            "GitHub release deletion",
            config,
            None,
        ).await;
        
        match delete_result {
            Ok(()) => {
                config.indent("   ✓ Deleted GitHub release");
            }
            Err(delete_err) => {
                // After retries, log warning but continue cleanup
                let warning = format!("Failed to delete GitHub release after retries: {}", delete_err);
                cleanup_warnings.push(warning.clone());
                config.indent(&format!("   ⚠️  {}", warning));
                config.indent(&format!("   ℹ️  If needed, manually delete: https://github.com/{}/{}/releases", owner, repo));
            }
        }
    }
    
    // 2. Rollback Git operations (Phase 2)
    config.indent("🔄 Rolling back git changes...");
    
    let rollback_result = retry_with_backoff(
        || async {
            git_manager.rollback_release().await.or_else(|e| {
                Ok(crate::git::RollbackResult {
                    success: false,
                    rolled_back_operations: Vec::new(),
                    warnings: vec![format!("Git rollback failed: {}", e)],
                    duration: std::time::Duration::from_secs(0),
                })
            })
        },
        config.retry_config.cleanup_operations,
        "Git rollback",
        config,
        None,
    ).await?;
    
    // Handle rollback result
    if rollback_result.success {
        config.indent("   ✓ Rolled back git changes:");
        for op in &rollback_result.rolled_back_operations {
            config.indent(&format!("     - {}", op));
        }
        if !rollback_result.warnings.is_empty() {
            for warning in &rollback_result.warnings {
                cleanup_warnings.push(warning.clone());
                config.indent(&format!("     ⚠️  {}", warning));
            }
        }
    } else {
        for warning in &rollback_result.warnings {
            cleanup_warnings.push(warning.clone());
            config.indent(&format!("   ⚠️  {}", warning));
        }
        config.indent("   ℹ️  If needed, manually run: git tag -d v{VERSION} && git push --delete origin v{VERSION}");
    }
    
    config.println("");
    if cleanup_warnings.is_empty() {
        config.success_println("✓ Cleanup completed successfully");
    } else {
        config.warning_println(&format!("⚠️  Cleanup completed with {} warning(s)", cleanup_warnings.len()));
    }
    
    // Show recovery suggestions from error
    let suggestions = e.recovery_suggestions();
    if !suggestions.is_empty() {
        config.println("");
        config.println("💡 Recovery suggestions:");
        for suggestion in suggestions {
            config.indent(&format!("  • {}", suggestion));
        }
    }
    
    config.println("");
    Err(e)
}

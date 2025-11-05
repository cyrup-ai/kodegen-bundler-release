//! Release implementation logic for isolated execution.
//!
//! This module contains the core release implementation that runs in an isolated
//! temporary clone to prevent modifications to the user's working directory.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, PublishError, ReleaseError, Result};
use crate::git::{GitConfig, GitManager};

use super::super::helpers::{create_bundles, detect_github_repo, parse_github_repo_string};
use super::ReleaseOptions;

/// Retry an async operation with exponential backoff
///
/// This helper automatically retries recoverable errors with intelligent backoff:
/// - Network/transient errors: Exponential backoff (1s, 2s, 4s, 8s)
/// - Rate limit errors: Wait exact time specified in error
/// - Unrecoverable errors: Return immediately without retry
///
/// # Arguments
/// * `operation` - Async closure that returns Result<T>
/// * `max_retries` - Maximum number of retry attempts (0 = try once, no retries)
/// * `operation_name` - Human-readable name for logging
/// * `config` - Runtime config for user messaging
///
/// # Returns
/// * `Ok(T)` - Operation succeeded (possibly after retries)
/// * `Err(ReleaseError)` - Operation failed after all retries, or unrecoverable error
async fn retry_with_backoff<F, T, Fut>(
    mut operation: F,
    max_retries: u32,
    operation_name: &str,
    config: &RuntimeConfig,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempts = 0;
    
    loop {
        match operation().await {
            Ok(result) => {
                if attempts > 0 {
                    config.success_println(&format!(
                        "✓ {} succeeded after {} retry(ies)",
                        operation_name,
                        attempts
                    ));
                }
                return Ok(result);
            }
            Err(e) => {
                // Check if error is recoverable
                if !e.is_recoverable() {
                    // Unrecoverable error - fail immediately, no retries
                    config.error_println(&format!(
                        "❌ {} failed with unrecoverable error",
                        operation_name
                    ));
                    return Err(e);
                }
                
                // Recoverable error - check if we have retries left
                if attempts >= max_retries {
                    // Retries exhausted
                    config.error_println(&format!(
                        "❌ {} failed after {} attempt(s)",
                        operation_name,
                        attempts + 1
                    ));
                    return Err(e);
                }
                
                attempts += 1;
                
                // Determine wait time based on error type
                let wait_seconds = match &e {
                    ReleaseError::Publish(PublishError::RateLimitExceeded { retry_after_seconds }) => {
                        // Use the exact wait time from the error
                        *retry_after_seconds
                    }
                    _ => {
                        // Exponential backoff: 1s, 2s, 4s, 8s, ...
                        2u64.pow(attempts - 1)
                    }
                };
                
                // Log retry attempt
                config.warning_println(&format!(
                    "⚠️  {} failed (attempt {}/{}): {}",
                    operation_name,
                    attempts,
                    max_retries + 1,
                    e
                ));
                config.indent(&format!("   Retrying in {}s...", wait_seconds));
                
                // Wait before retry
                tokio::time::sleep(tokio::time::Duration::from_secs(wait_seconds)).await;
            }
        }
    }
}

/// Execute release phases 2-8 with retry logic
///
/// This function handles all phases that involve network operations and may need retry logic.
/// Phase 1 (version bump) and Phase 1.5 (conflict cleanup) are handled separately.
async fn execute_phases_with_retry(
    temp_dir: &std::path::Path,
    metadata: &crate::metadata::PackageMetadata,
    binary_name: &str,
    new_version: &semver::Version,
    config: &RuntimeConfig,
    options: &ReleaseOptions,
    git_manager: &mut GitManager,
    github_manager: &crate::github::GitHubReleaseManager,
    github_release_id: &mut Option<u64>,
    _owner: &str,
    _repo: &str,
) -> Result<()> {
    // ===== PHASE 2: GIT OPERATIONS (with retry) =====
    config.println("📝 Creating git commit...");
    
    // Manual retry loop for git operations (can't use retry_with_backoff due to &mut borrow)
    let git_result = {
        let max_retries = 3u32;
        let mut attempts = 0;
        
        loop {
            match git_manager.perform_release(new_version, !options.no_push).await {
                Ok(result) => {
                    if attempts > 0 {
                        config.success_println(&format!(
                            "✓ Git operations succeeded after {} retry(ies)",
                            attempts
                        ));
                    }
                    break result;
                }
                Err(e) => {
                    // Check if error is recoverable
                    if !e.is_recoverable() {
                        config.error_println("❌ Git operations failed with unrecoverable error");
                        return Err(e);
                    }
                    
                    // Recoverable error - check if we have retries left
                    if attempts >= max_retries {
                        config.error_println(&format!(
                            "❌ Git operations failed after {} attempt(s)",
                            attempts + 1
                        ));
                        return Err(e);
                    }
                    
                    attempts += 1;
                    
                    // Exponential backoff: 1s, 2s, 4s, 8s, ...
                    let wait_seconds = 2u64.pow(attempts - 1);
                    
                    config.warning_println(&format!(
                        "⚠️  Git operations failed (attempt {}/{}): {}",
                        attempts,
                        max_retries + 1,
                        e
                    ));
                    config.indent(&format!("   Retrying in {}s...", wait_seconds));
                    
                    tokio::time::sleep(tokio::time::Duration::from_secs(wait_seconds)).await;
                }
            }
        }
    };
    
    config.success_println(&format!("✓ Committed: \"{}\"", git_result.commit.message));
    config.success_println(&format!("✓ Tagged: {}", git_result.tag.name));
    if !options.no_push {
        config.success_println("✓ Pushed to origin");
    }
    
    // ===== PHASE 3: CREATE GITHUB DRAFT RELEASE (with retry) =====
    config.println("🚀 Creating GitHub draft release...");
    
    let release_result = retry_with_backoff(
        || github_manager.create_release(
            new_version,
            &git_result.commit.hash,
            None,
        ),
        3,  // Max 3 retries for GitHub API calls
        "GitHub release creation",
        config,
    ).await?;
    
    config.success_println(&format!("✓ Created draft release: {}", release_result.html_url));
    
    // Track release ID for potential cleanup
    *github_release_id = Some(release_result.release_id);
    let release_id = release_result.release_id;
    
    // ===== PHASE 4: BUILD BINARY (no retry - build errors are deterministic) =====
    if options.universal && cfg!(target_os = "macos") {
        config.println(&format!("🔨 Building universal binary '{}' (x86_64 + arm64)...", binary_name));
        
        // Build for Intel (x86_64)
        config.verbose_println("  Building for x86_64 (Intel)...");
        super::super::helpers::build_for_target(
            temp_dir, 
            binary_name, 
            "x86_64-apple-darwin", 
            config
        )?;
        
        // Build for Apple Silicon (arm64)
        config.verbose_println("  Building for aarch64 (Apple Silicon)...");
        super::super::helpers::build_for_target(
            temp_dir, 
            binary_name, 
            "aarch64-apple-darwin", 
            config
        )?;
        
        // Create universal binaries using lipo
        config.verbose_println("  Merging architectures with lipo...");
        let output_dir = temp_dir.join("target/universal/release");
        let universal_binaries = crate::bundler::platform::macos::universal::create_universal_binaries(
            temp_dir,
            &output_dir,
        )?;
        
        // Copy universal binary to target/release for bundler pickup
        let release_dir = temp_dir.join("target/release");
        std::fs::create_dir_all(&release_dir).map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "create_release_dir".to_string(),
                reason: format!("Failed to create release directory {}: {}", release_dir.display(), e),
            })
        })?;
        
        for universal_bin in &universal_binaries {
            if let Some(filename) = universal_bin.file_name() {
                let dest = release_dir.join(filename);
                std::fs::copy(universal_bin, &dest).map_err(|e| {
                    ReleaseError::Cli(CliError::ExecutionFailed {
                        command: "copy_universal_binary".to_string(),
                        reason: format!("Failed to copy {} to release dir: {}", filename.to_string_lossy(), e),
                    })
                })?;
                config.verbose_println(&format!("  Copied {} to target/release/", filename.to_string_lossy()));
            }
        }
        
        config.success_println("✓ Universal binary created (supports Intel + Apple Silicon)");
    } else if options.universal && !cfg!(target_os = "macos") {
        config.warning_println("⚠️  --universal flag ignored (only supported on macOS)");
        config.println(&format!("🔨 Building binary '{}'...", binary_name));
        super::super::helpers::build_binary(temp_dir, binary_name, true, config)?;
        config.success_println("✓ Build complete");
    } else {
        config.println(&format!("🔨 Building binary '{}'...", binary_name));
        super::super::helpers::build_binary(temp_dir, binary_name, true, config)?;
        config.success_println("✓ Build complete");
    }
    
    // ===== PHASE 5: CREATE PLATFORM PACKAGES (no retry - local operation) =====
    config.println("📦 Creating platform installers...");
    
    let bundled_artifacts = create_bundles(
        temp_dir,
        metadata,
        binary_name,
        new_version,
        config,
    ).await?;
    
    config.success_println(&format!("✓ Created {} platform package(s)", bundled_artifacts.len()));
    
    // ===== PHASE 6: UPLOAD PACKAGES (with retry per file) =====
    config.println("📤 Uploading packages to GitHub...");
    config.indent(&format!("   Release: {}", release_result.html_url));
    config.println("");
    
    let mut upload_count = 0;
    for artifact in &bundled_artifacts {
        for artifact_path in &artifact.paths {
            // Skip directories (e.g., .app bundles - only upload files)
            if !artifact_path.is_file() {
                continue;
            }
            
            let filename = artifact_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            
            config.indent(&format!("Uploading {}...", filename));
            
            // Upload with retry
            let download_urls = retry_with_backoff(
                || github_manager.upload_artifacts(
                    release_id,
                    std::slice::from_ref(artifact_path),
                    config,
                ),
                3,  // Max 3 retries per file upload
                &format!("Upload {}", filename),
                config,
            ).await?;
            
            config.indent(&format!("✓ Uploaded {}", filename));
            
            // Display each download URL
            for url in &download_urls {
                config.indent(&format!("   📥 {}", url));
            }
            
            upload_count += 1;
        }
    }
    
    config.success_println(&format!("✓ Uploaded {} artifact(s)", upload_count));
    
    // ===== PHASE 7: PUBLISH RELEASE (with retry) =====
    config.println("✅ Publishing GitHub release...");
    
    retry_with_backoff(
        || github_manager.publish_draft_release(release_id),
        3,  // Max 3 retries for publish
        "Publish GitHub release",
        config,
    ).await?;
    
    config.success_println(&format!("✓ Published release v{}", new_version));
    
    // ===== PHASE 8: PUBLISH TO CRATES.IO (with retry) =====
    if let Some(registry) = &options.registry {
        config.println(&format!("📦 Publishing to {}...", registry));
        
        let publish_result = retry_with_backoff(
            || async {
                let output = std::process::Command::new("cargo")
                    .arg("publish")
                    .arg("--registry")
                    .arg(registry)
                    .current_dir(temp_dir)
                    .output()
                    .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                        command: "cargo_publish".to_string(),
                        reason: e.to_string(),
                    }))?;
                
                if output.status.success() {
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(ReleaseError::Publish(PublishError::PublishFailed {
                        package: metadata.name.clone(),
                        reason: stderr.to_string(),
                    }))
                }
            },
            2,  // Max 2 retries for crates.io (rate limits usually need only one wait)
            &format!("Publish to {}", registry),
            config,
        ).await;
        
        match publish_result {
            Ok(()) => {
                config.success_println(&format!("✓ Published {} v{} to {}", metadata.name, new_version, registry));
            }
            Err(e) => {
                config.warning_println(&format!("⚠️  Publishing failed: {}", e));
                // Continue anyway - GitHub release is already published
            }
        }
    } else {
        config.println("📦 Publishing to crates.io...");
        
        let publish_result = retry_with_backoff(
            || async {
                let output = std::process::Command::new("cargo")
                    .arg("publish")
                    .current_dir(temp_dir)
                    .output()
                    .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                        command: "cargo_publish".to_string(),
                        reason: e.to_string(),
                    }))?;
                
                if output.status.success() {
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(ReleaseError::Publish(PublishError::PublishFailed {
                        package: metadata.name.clone(),
                        reason: stderr.to_string(),
                    }))
                }
            },
            2,  // Max 2 retries for crates.io
            "Publish to crates.io",
            config,
        ).await;
        
        match publish_result {
            Ok(()) => {
                config.success_println(&format!("✓ Published {} v{} to crates.io", metadata.name, new_version));
            }
            Err(_) => {
                config.verbose_println("ℹ️  Skipping crates.io publish (may not be a library crate)");
            }
        }
    }
    
    Ok(())
}

/// Perform release for a single repository (not a workspace).
///
/// This is the simplified release flow for individual packages.
/// Replaces the workspace-aware perform_release_impl().
pub(super) async fn perform_release_single_repo(
    temp_dir: &std::path::Path,
    metadata: crate::metadata::PackageMetadata,
    binary_name: String,
    config: &RuntimeConfig,
    options: &ReleaseOptions,
) -> Result<i32> {
    config.println("🚀 Starting release in isolated environment");
    
    // ===== PHASE 1: VERSION BUMP =====
    config.println("🔢 Bumping version...");
    
    let cargo_toml_path = temp_dir.join("Cargo.toml");
    let current_version = semver::Version::parse(&metadata.version)
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
            version: metadata.version.clone(),
            reason: e.to_string(),
        }))?;
    
    let version_bump = crate::version::VersionBump::try_from(options.bump_type.clone())
        .map_err(|e| ReleaseError::Cli(CliError::InvalidArguments { reason: e }))?;
    let bumper = crate::version::VersionBumper::from_version(current_version.clone());
    let new_version = bumper.bump(version_bump.clone())?;
    
    config.success_println(&format!("✓ v{} → v{} ({})", current_version, new_version, version_bump));
    
    // Update Cargo.toml with new version
    crate::version::update_single_toml(&cargo_toml_path, &new_version.to_string())?;
    config.success_println("✓ Updated Cargo.toml");
    
    // ===== PHASE 1.5: AUTOMATIC CLEANUP OF CONFLICTS =====
    config.println("🔍 Checking for conflicting artifacts...");
    
    // Create git_manager early for cleanup
    let git_config = GitConfig {
        default_remote: "origin".to_string(),
        annotated_tags: true,
        auto_push_tags: !options.no_push,
        ..Default::default()
    };
    let mut git_manager = GitManager::with_config(temp_dir, git_config).await?;
    
    // Validate working directory is clean (can't proceed if dirty)
    use crate::error::GitError;
    if !git_manager.is_clean().await? {
        return Err(ReleaseError::Git(GitError::DirtyWorkingDirectory));
    }
    
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
    
    // Track GitHub release ID for cleanup (None until Phase 3 succeeds)
    let mut github_release_id: Option<u64> = None;
    
    // ===== EXECUTE PHASES 2-8 WITH RETRY AND SELECTIVE CLEANUP =====
    let result = execute_phases_with_retry(
        temp_dir,
        &metadata,
        &binary_name,
        &new_version,
        config,
        options,
        &mut git_manager,
        &github_manager,
        &mut github_release_id,
        &owner,
        &repo,
    ).await;
    
    match result {
        Ok(()) => {
            // SUCCESS: Clear state and return
            git_manager.clear_release_state();
            Ok(0)
        }
        Err(e) if !e.is_recoverable() => {
            // UNRECOVERABLE ERROR: Cleanup required
            config.println("");
            config.error_println(&format!("❌ Release failed with unrecoverable error: {}", e));
            config.println("");
            config.println("🧹 Rolling back changes...");
            
            let mut cleanup_warnings = Vec::new();
            
            // 1. Delete GitHub release if created (Phase 3+)
            if let Some(release_id) = github_release_id {
                config.indent("🗑️  Deleting GitHub draft release...");
                match github_manager.delete_release(release_id).await {
                    Ok(()) => {
                        config.indent("   ✓ Deleted GitHub release");
                    }
                    Err(delete_err) => {
                        let warning = format!("Failed to delete GitHub release: {}", delete_err);
                        cleanup_warnings.push(warning.clone());
                        config.indent(&format!("   ⚠️  {}", warning));
                        config.indent(&format!("   ℹ️  Manual cleanup: https://github.com/{}/{}/releases", owner, repo));
                    }
                }
            }
            
            // 2. Rollback Git operations (Phase 2)
            config.indent("🔄 Rolling back git changes...");
            match git_manager.rollback_release().await {
                Ok(rollback_result) => {
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
                    }
                }
                Err(rollback_err) => {
                    let warning = format!("Git rollback failed: {}", rollback_err);
                    cleanup_warnings.push(warning.clone());
                    config.indent(&format!("   ⚠️  {}", warning));
                    config.indent("   ℹ️  Manual cleanup may be required: git tag -d, git push --delete origin");
                }
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
        Err(e) => {
            // RECOVERABLE ERROR (retries exhausted) - No cleanup needed
            // The error is transient (network, timeout, etc.) - retrying might work next time
            // Don't cleanup git/GitHub artifacts as they're valid, just incomplete
            config.println("");
            config.error_println(&format!("❌ Release failed after retries: {}", e));
            config.println("");
            config.println("ℹ️  This appears to be a transient error (network, timeout, etc.)");
            config.println("ℹ️  Retrying the release command may succeed.");
            config.println("ℹ️  No cleanup performed - partial artifacts may exist.");
            config.println("");
            Err(e)
        }
    }
}

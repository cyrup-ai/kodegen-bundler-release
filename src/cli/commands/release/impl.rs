//! Release implementation logic for isolated execution.
//!
//! This module contains the core release implementation that runs in an isolated
//! temporary clone to prevent modifications to the user's working directory.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, PublishError, ReleaseError, Result};
use crate::git::{GitConfig, GitManager};
use crate::state::ReleaseState;

use super::super::helpers::{create_bundles, detect_github_repo, parse_github_repo_string};
use super::ReleaseOptions;

/// Context for executing release phases with all required dependencies
struct ReleasePhaseContext<'a> {
    /// Temporary directory for isolated execution
    temp_dir: &'a std::path::Path,
    /// Package metadata from Cargo.toml
    metadata: &'a crate::metadata::PackageMetadata,
    /// Binary name to build and release
    binary_name: &'a str,
    /// Target version for this release
    new_version: &'a semver::Version,
    /// Runtime configuration for output and settings
    config: &'a RuntimeConfig,
    /// Release-specific options (bump type, push behavior, etc.)
    options: &'a ReleaseOptions,
    /// Git manager for version control operations
    git_manager: &'a GitManager,
    /// GitHub manager for release and artifact management
    github_manager: &'a crate::github::GitHubReleaseManager,
    /// GitHub repository owner
    owner: &'a str,
    /// GitHub repository name
    repo: &'a str,
}

/// Maximum backoff time in seconds (1 hour)
/// Prevents exponential backoff from producing impractical wait times
const MAX_BACKOFF_SECONDS: u64 = 3600;

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
                        // Use the exact wait time from the error (but still cap it)
                        (*retry_after_seconds).min(MAX_BACKOFF_SECONDS)
                    }
                    _ => {
                        // Exponential backoff with overflow protection: 1s, 2s, 4s, 8s, ..., max 3600s
                        // Use saturating_pow to prevent panic, then cap at maximum
                        2u64.saturating_pow(attempts - 1).min(MAX_BACKOFF_SECONDS)
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
    ctx: &ReleasePhaseContext<'_>,
    release_state: &mut ReleaseState,
) -> Result<()> {
    // ===== PHASE 2: GIT OPERATIONS (with retry) =====
    let git_result: Option<crate::git::ReleaseResult> = if release_state.has_completed(crate::state::ReleasePhase::GitOperations) {
        ctx.config.println("✓ Skipping git operations (already completed)");
        if let Some(ref git_state) = release_state.git_state {
            if let Some(ref commit) = git_state.release_commit {
                ctx.config.indent(&format!("   Commit: {}", commit.short_hash));
            }
            if let Some(ref tag) = git_state.release_tag {
                ctx.config.indent(&format!("   Tag: {}", tag.name));
            }
        }
        None
    } else {
        ctx.config.println("📝 Creating git commit...");
        
        let result = retry_with_backoff(
            || ctx.git_manager.perform_release(ctx.new_version, !ctx.options.no_push),
            ctx.config.retry_config.git_operations,
            "Git operations",
            ctx.config,
        ).await?;
        
        ctx.config.success_println(&format!("✓ Committed: \"{}\"", result.commit.message));
        ctx.config.success_println(&format!("✓ Tagged: {}", result.tag.name));
        if !ctx.options.no_push {
            ctx.config.success_println("✓ Pushed to origin");
        }
        
        // Save state after git operations complete
        release_state.set_phase(crate::state::ReleasePhase::GitOperations);
        release_state.add_checkpoint(
            "git_operations_complete".to_string(),
            crate::state::ReleasePhase::GitOperations,
            None,
            true,  // rollback_capable
        );
        crate::state::save_release_state(release_state).await?;
        ctx.config.verbose_println("ℹ️  Saved progress checkpoint (Git operations)");
        
        Some(result)
    };
    
    // ===== PHASE 3 PRECHECK: VERIFY GITHUB API ACCESS =====
    ctx.config.println("🔍 Verifying GitHub API access...");

    if !ctx.github_manager.test_connection().await? {
        return Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: "GitHub API authentication failed. Check GH_TOKEN or GITHUB_TOKEN environment variable.".to_string(),
        }));
    }

    ctx.config.success_println("✓ GitHub API authenticated");
    ctx.config.println("");

    // ===== PHASE 3: CREATE GITHUB DRAFT RELEASE (with retry) =====
    let release_id = if release_state.has_completed(crate::state::ReleasePhase::GitHubRelease) {
        ctx.config.println("✓ Skipping GitHub release creation (already completed)");
        if let Some(ref github_state) = release_state.github_state {
            ctx.config.indent(&format!("   Release: {}", github_state.html_url.as_ref().unwrap_or(&"N/A".to_string())));
            github_state.release_id.ok_or_else(|| {
                ReleaseError::State(crate::error::StateError::Corrupted {
                    reason: "GitHubRelease checkpoint exists but release_id is None".to_string(),
                })
            })?
        } else {
            return Err(ReleaseError::State(crate::error::StateError::Corrupted {
                reason: "GitHubRelease checkpoint exists but github_state is None".to_string(),
            }));
        }
    } else {
        ctx.config.println("🚀 Creating GitHub draft release...");
        
        // Get commit hash from git_result or from stored state
        let commit_hash = if let Some(ref result) = git_result {
            result.commit.hash.clone()
        } else if let Some(ref git_state) = release_state.git_state {
            git_state.release_commit.as_ref()
                .ok_or_else(|| ReleaseError::State(crate::error::StateError::Corrupted {
                    reason: "Git operations completed but release_commit is None".to_string(),
                }))?
                .hash.clone()
        } else {
            return Err(ReleaseError::State(crate::error::StateError::Corrupted {
                reason: "Cannot create GitHub release: no git commit information available".to_string(),
            }));
        };
        
        let release_result = retry_with_backoff(
            || ctx.github_manager.create_release(
                ctx.new_version,
                &commit_hash,
                None,
            ),
            ctx.config.retry_config.github_api,
            "GitHub release creation",
            ctx.config,
        ).await?;
        
        ctx.config.success_println(&format!("✓ Created draft release: {}", release_result.html_url));
        
        // Track release in state for potential cleanup
        release_state.set_github_state(ctx.owner.to_string(), ctx.repo.to_string(), Some(&release_result));
        let release_id = release_result.release_id;
        
        // Save state after GitHub release created
        release_state.set_phase(crate::state::ReleasePhase::GitHubRelease);
        release_state.add_checkpoint(
            "github_release_created".to_string(),
            crate::state::ReleasePhase::GitHubRelease,
            Some(serde_json::json!({
                "release_id": release_id,
                "html_url": &release_result.html_url,
            })),
            true,  // rollback_capable
        );
        crate::state::save_release_state(release_state).await?;
        ctx.config.verbose_println("ℹ️  Saved progress checkpoint (GitHub release)");
        
        release_id
    };
    
    // ===== PHASE 4 PRECHECK: VERIFY BUILD TOOLS =====
    ctx.config.println("🔍 Checking build tools...");

    if !super::super::helpers::check_cargo_available() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "cargo".to_string(),
            reason: "cargo not found. Install Rust toolchain: https://rustup.rs/".to_string(),
        }));
    }

    if !super::super::helpers::check_rustc_available() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "rustc".to_string(),
            reason: "rustc not found. Install Rust toolchain: https://rustup.rs/".to_string(),
        }));
    }

    ctx.config.success_println("✓ Build tools available");
    ctx.config.println("");

    // ===== PHASE 4: BUILD BINARY (no retry - build errors are deterministic) =====
    if ctx.options.universal && cfg!(target_os = "macos") {
        ctx.config.println(&format!("🔨 Building universal binary '{}' (x86_64 + arm64)...", ctx.binary_name));
        
        // Build for Intel (x86_64)
        ctx.config.verbose_println("  Building for x86_64 (Intel)...");
        super::super::helpers::build_for_target(
            ctx.temp_dir, 
            ctx.binary_name, 
            "x86_64-apple-darwin", 
            ctx.config
        )?;
        
        // Build for Apple Silicon (arm64)
        ctx.config.verbose_println("  Building for aarch64 (Apple Silicon)...");
        super::super::helpers::build_for_target(
            ctx.temp_dir, 
            ctx.binary_name, 
            "aarch64-apple-darwin", 
            ctx.config
        )?;
        
        // Create universal binaries using lipo
        ctx.config.verbose_println("  Merging architectures with lipo...");
        let output_dir = ctx.temp_dir.join("target/universal/release");
        let universal_binaries = crate::bundler::platform::macos::universal::create_universal_binaries(
            ctx.temp_dir,
            &output_dir,
        )?;
        
        // Copy universal binary to target/release for bundler pickup
        let release_dir = ctx.temp_dir.join("target/release");
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
                ctx.config.verbose_println(&format!("  Copied {} to target/release/", filename.to_string_lossy()));
            }
        }
        
        ctx.config.success_println("✓ Universal binary created (supports Intel + Apple Silicon)");
    } else if ctx.options.universal && !cfg!(target_os = "macos") {
        ctx.config.warning_println("⚠️  --universal flag ignored (only supported on macOS)");
        ctx.config.println(&format!("🔨 Building binary '{}'...", ctx.binary_name));
        super::super::helpers::build_binary(ctx.temp_dir, ctx.binary_name, true, ctx.config)?;
        ctx.config.success_println("✓ Build complete");
    } else {
        ctx.config.println(&format!("🔨 Building binary '{}'...", ctx.binary_name));
        super::super::helpers::build_binary(ctx.temp_dir, ctx.binary_name, true, ctx.config)?;
        ctx.config.success_println("✓ Build complete");
    }
    
    // ===== PHASE 5: CREATE PLATFORM PACKAGES (no retry - local operation) =====
    ctx.config.println("📦 Creating platform installers...");
    
    let bundled_artifacts = create_bundles(
        ctx.temp_dir,
        ctx.metadata,
        ctx.binary_name,
        ctx.new_version,
        ctx.config,
    ).await?;
    
    ctx.config.success_println(&format!("✓ Created {} platform package(s)", bundled_artifacts.len()));
    
    // ===== PHASE 6: UPLOAD PACKAGES (with retry per file) =====
    if release_state.has_completed(crate::state::ReleasePhase::GitHubRelease) 
        && release_state.checkpoints.iter().any(|cp| cp.name == "artifacts_uploaded") {
        ctx.config.println("✓ Skipping artifact upload (already completed)");
    } else {
        ctx.config.println("📤 Uploading packages to GitHub...");
        ctx.config.println("");
        
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
                
                ctx.config.indent(&format!("Uploading {}...", filename));
                
                // Upload with retry
                let download_urls = retry_with_backoff(
                    || ctx.github_manager.upload_artifacts(
                        release_id,
                        std::slice::from_ref(artifact_path),
                        ctx.new_version,
                        ctx.config,
                    ),
                    ctx.config.retry_config.file_uploads,
                    &format!("Upload {}", filename),
                    ctx.config,
                ).await?;
                
                ctx.config.indent(&format!("✓ Uploaded {}", filename));
                
                // Display each download URL
                for url in &download_urls {
                    ctx.config.indent(&format!("   📥 {}", url));
                }
                
                upload_count += 1;
            }
        }
        
        ctx.config.success_println(&format!("✓ Uploaded {} artifact(s)", upload_count));
        
        // Save state after all uploads complete
        release_state.set_phase(crate::state::ReleasePhase::GitHubRelease);  // Still in release phase
        release_state.add_checkpoint(
            "artifacts_uploaded".to_string(),
            crate::state::ReleasePhase::GitHubRelease,
            Some(serde_json::json!({
                "upload_count": upload_count,
            })),
            false,  // Not rollback-capable (can't delete uploaded files easily)
        );
        crate::state::save_release_state(release_state).await?;
        ctx.config.verbose_println("ℹ️  Saved progress checkpoint (Artifacts uploaded)");
    }
    
    // ===== PHASE 7: PUBLISH RELEASE (with retry) =====
    if release_state.has_completed(crate::state::ReleasePhase::GitHubPublish) {
        ctx.config.println("✓ Skipping release publishing (already published)");
    } else {
        ctx.config.println("🔍 Verifying release is ready to publish...");

        if !ctx.github_manager.verify_release_is_draft(release_id).await? {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "publish_release".to_string(),
                reason: format!(
                    "Release {} is not a draft or was deleted. Cannot publish.",
                    release_id
                ),
            }));
        }

        ctx.config.success_println("✓ Release verified as draft");
        ctx.config.println("");

        ctx.config.println("✅ Publishing GitHub release...");
        
        retry_with_backoff(
            || ctx.github_manager.publish_draft_release(release_id),
            ctx.config.retry_config.release_publishing,
            "Publish GitHub release",
            ctx.config,
        ).await?;
        
        ctx.config.success_println(&format!("✓ Published release v{}", ctx.new_version));
        
        // Save state after publishing
        release_state.set_phase(crate::state::ReleasePhase::GitHubPublish);
        release_state.add_checkpoint(
            "release_published".to_string(),
            crate::state::ReleasePhase::GitHubPublish,
            None,
            false,  // Can't unpublish
        );
        crate::state::save_release_state(release_state).await?;
        ctx.config.verbose_println("ℹ️  Saved progress checkpoint (Release published)");
    }
    
    // ===== PHASE 8: PUBLISH TO CRATES.IO (with retry) =====
    if let Some(registry) = &ctx.options.registry {
        ctx.config.println(&format!("📦 Publishing to {}...", registry));
        
        let publish_result = retry_with_backoff(
            || async {
                let output = std::process::Command::new("cargo")
                    .arg("publish")
                    .arg("--registry")
                    .arg(registry)
                    .current_dir(ctx.temp_dir)
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
                        package: ctx.metadata.name.clone(),
                        reason: stderr.to_string(),
                    }))
                }
            },
            ctx.config.retry_config.release_publishing,
            &format!("Publish to {}", registry),
            ctx.config,
        ).await;
        
        match publish_result {
            Ok(()) => {
                ctx.config.success_println(&format!("✓ Published {} v{} to {}", ctx.metadata.name, ctx.new_version, registry));
            }
            Err(e) => {
                ctx.config.warning_println(&format!("⚠️  Publishing failed: {}", e));
                // Continue anyway - GitHub release is already published
            }
        }
    } else {
        ctx.config.println("📦 Publishing to crates.io...");
        
        let publish_result = retry_with_backoff(
            || async {
                let output = std::process::Command::new("cargo")
                    .arg("publish")
                    .current_dir(ctx.temp_dir)
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
                        package: ctx.metadata.name.clone(),
                        reason: stderr.to_string(),
                    }))
                }
            },
            ctx.config.retry_config.release_publishing,
            "Publish to crates.io",
            ctx.config,
        ).await;
        
        match publish_result {
            Ok(()) => {
                ctx.config.success_println(&format!("✓ Published {} v{} to crates.io", ctx.metadata.name, ctx.new_version));
            }
            Err(_) => {
                ctx.config.verbose_println("ℹ️  Skipping crates.io publish (may not be a library crate)");
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
    
    // ===== LOAD OR CREATE RELEASE STATE =====
    use crate::state::{has_active_release, load_release_state, LoadStateResult};

    let mut release_state = if has_active_release() {
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
                    .map_err(|e| ReleaseError::Cli(CliError::InvalidArguments { reason: e }))?;
                
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
                    ReleaseState::new(
                        expected_version,
                        version_bump,
                        crate::state::ReleaseConfig::default(),
                    )
                } else {
                    config.success_println(&format!("✓ Resuming release v{}", state.target_version));
                    config.indent(&format!("   Current phase: {:?}", state.current_phase));
                    config.indent(&format!("   Checkpoints: {}", state.checkpoints.len()));
                    state
                }
            }
            Err(e) => {
                config.warning_println(&format!("⚠️  Failed to load state: {}", e));
                config.warning_println("   Starting fresh release...");
                
                // Create new state
                let current_version = semver::Version::parse(&metadata.version)
                    .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
                        version: metadata.version.clone(),
                        reason: e.to_string(),
                    }))?;
                
                let version_bump = crate::version::VersionBump::try_from(options.bump_type.clone())
                    .map_err(|e| ReleaseError::Cli(CliError::InvalidArguments { reason: e }))?;
                
                let bumper = crate::version::VersionBumper::from_version(current_version.clone());
                let new_version = bumper.bump(version_bump.clone())?;
                
                ReleaseState::new(
                    new_version,
                    version_bump,
                    crate::state::ReleaseConfig::default(),
                )
            }
        }
    } else {
        // No existing state - start fresh
        let current_version = semver::Version::parse(&metadata.version)
            .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
                version: metadata.version.clone(),
                reason: e.to_string(),
            }))?;
        
        let version_bump = crate::version::VersionBump::try_from(options.bump_type.clone())
            .map_err(|e| ReleaseError::Cli(CliError::InvalidArguments { reason: e }))?;
        
        let bumper = crate::version::VersionBumper::from_version(current_version.clone());
        let new_version = bumper.bump(version_bump.clone())?;
        
        ReleaseState::new(
            new_version,
            version_bump,
            crate::state::ReleaseConfig::default(),
        )
    };
    
    // Extract version information for use in subsequent code
    let new_version = release_state.target_version.clone();
    let version_bump = release_state.version_bump.clone();
    
    // Parse current version from metadata for display
    let current_version = semver::Version::parse(&metadata.version)
        .map_err(|e| ReleaseError::Version(crate::error::VersionError::InvalidVersion {
            version: metadata.version.clone(),
            reason: e.to_string(),
        }))?;
    
    // ===== PHASE 1: VERSION BUMP =====
    config.println("🔢 Bumping version...");
    
    let cargo_toml_path = temp_dir.join("Cargo.toml");
    
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
    let git_manager = GitManager::with_config(temp_dir, git_config).await?;
    
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
    
    // ===== EXECUTE PHASES 2-8 WITH RETRY AND SELECTIVE CLEANUP =====
    // Create context for phase execution
    let phase_ctx = ReleasePhaseContext {
        temp_dir,
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
                ).await;
                
                match delete_result {
                    Ok(()) => {
                        config.indent("   ✓ Deleted GitHub release");
                    }
                    Err(delete_err) => {
                        // After 3 retries, log warning but continue cleanup
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
            ).await?;
            
            // Handle rollback result (same as before)
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

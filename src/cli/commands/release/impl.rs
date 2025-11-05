//! Release implementation logic for isolated execution.
//!
//! This module contains the core release implementation that runs in an isolated
//! temporary clone to prevent modifications to the user's working directory.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, ReleaseError, Result};
use crate::git::{GitConfig, GitManager};

use super::super::helpers::{create_bundles, detect_github_repo, parse_github_repo_string};
use super::ReleaseOptions;

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
    
    // ===== PHASE 2: GIT OPERATIONS =====
    config.println("📝 Creating git commit...");
    
    // git_manager already created in Phase 1.5 - just use it
    let git_result = git_manager.perform_release(&new_version, !options.no_push).await?;
    
    config.success_println(&format!("✓ Committed: \"{}\"", git_result.commit.message));
    config.success_println(&format!("✓ Tagged: {}", git_result.tag.name));
    if !options.no_push {
        config.success_println("✓ Pushed to origin");
    }
    
    // ===== PHASE 3: CREATE GITHUB DRAFT RELEASE =====
    config.println("🚀 Creating GitHub draft release...");
    
    // github_manager already created in Phase 1.5 - just use it
    let release_result = github_manager.create_release(
        &new_version,
        &git_result.commit.hash,
        None,  // Auto-generated notes
    ).await?;
    
    config.success_println(&format!("✓ Created draft release: {}", release_result.html_url));
    let release_id = release_result.release_id;
    
    // ===== PHASE 4: BUILD BINARY =====
    if options.universal && cfg!(target_os = "macos") {
        config.println(&format!("🔨 Building universal binary '{}' (x86_64 + arm64)...", binary_name));
        
        // Build for Intel (x86_64)
        config.verbose_println("  Building for x86_64 (Intel)...");
        super::super::helpers::build_for_target(
            temp_dir, 
            &binary_name, 
            "x86_64-apple-darwin", 
            config
        )?;
        
        // Build for Apple Silicon (arm64)
        config.verbose_println("  Building for aarch64 (Apple Silicon)...");
        super::super::helpers::build_for_target(
            temp_dir, 
            &binary_name, 
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
        // (bundler expects binaries at target/release/)
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
        // User requested universal but not on macOS - inform and continue
        config.warning_println("⚠️  --universal flag ignored (only supported on macOS)");
        config.println(&format!("🔨 Building binary '{}'...", binary_name));
        super::super::helpers::build_binary(temp_dir, &binary_name, true, config)?;
        config.success_println("✓ Build complete");
    } else {
        // Standard single-architecture build
        config.println(&format!("🔨 Building binary '{}'...", binary_name));
        super::super::helpers::build_binary(temp_dir, &binary_name, true, config)?;
        config.success_println("✓ Build complete");
    }
    
    // ===== PHASE 5: CREATE PLATFORM PACKAGES =====
    config.println("📦 Creating platform installers...");
    
    let bundled_artifacts = create_bundles(
        temp_dir,
        &metadata,
        &binary_name,
        &new_version,
        config,
    ).await?;
    
    config.success_println(&format!("✓ Created {} platform package(s)", bundled_artifacts.len()));
    
    // ===== PHASE 6: UPLOAD PACKAGES INCREMENTALLY =====
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
            
            // Upload immediately and CAPTURE download URLs
            let download_urls = github_manager.upload_artifacts(
                release_id,
                std::slice::from_ref(artifact_path),
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
    
    // ===== PHASE 7: PUBLISH RELEASE =====
    config.println("✅ Publishing GitHub release...");
    
    github_manager.publish_draft_release(release_id).await?;
    config.success_println(&format!("✓ Published release v{}", new_version));
    
    // ===== PHASE 8: PUBLISH TO CRATES.IO (optional) =====
    if let Some(registry) = &options.registry {
        config.println(&format!("📦 Publishing to {}...", registry));
        
        let publish_result = std::process::Command::new("cargo")
            .arg("publish")
            .arg("--registry")
            .arg(registry)
            .current_dir(temp_dir)
            .output()
            .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                command: "cargo_publish".to_string(),
                reason: e.to_string(),
            }))?;
        
        if publish_result.status.success() {
            config.success_println(&format!("✓ Published {} v{} to {}", metadata.name, new_version, registry));
        } else {
            let stderr = String::from_utf8_lossy(&publish_result.stderr);
            config.warning_println(&format!("⚠️  Publishing failed: {}", stderr));
            // Continue anyway - GitHub release is already published
        }
    } else {
        config.println("📦 Publishing to crates.io...");
        
        let publish_result = std::process::Command::new("cargo")
            .arg("publish")
            .current_dir(temp_dir)
            .output()
            .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                command: "cargo_publish".to_string(),
                reason: e.to_string(),
            }))?;
        
        if publish_result.status.success() {
            config.success_println(&format!("✓ Published {} v{} to crates.io", metadata.name, new_version));
        } else {
            config.verbose_println("ℹ️  Skipping crates.io publish (may not be a library crate)");
        }
    }
    
    // ===== SUCCESS =====
    config.println("");
    config.success_println(&format!("✅ Successfully released v{}", new_version));
    config.println(&format!("   GitHub: {}", release_result.html_url));
    config.println(&format!("   crates.io: https://crates.io/crates/{}/{}", metadata.name, new_version));
    
    Ok(0)
}

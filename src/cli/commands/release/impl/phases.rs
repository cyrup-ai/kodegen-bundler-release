//! Release phase execution with retry logic.
//!
//! This module orchestrates phases 2-8 of the release process with intelligent
//! retry logic for network operations.

use crate::error::{CliError, ReleaseError, Result};
use crate::state::ReleaseState;

use super::context::ReleasePhaseContext;
use super::platform::{
    bundle_docker_platform, bundle_native_platform, ensure_bundler_installed,
    get_docker_platforms, get_native_platforms, get_platforms_to_build,
};
use super::retry::retry_with_backoff;

/// Execute release phases 2-8 with retry logic
///
/// This function handles all phases that involve network operations and may need retry logic.
/// Phase 1 (version bump) and Phase 1.5 (conflict cleanup) are handled separately.
pub async fn execute_phases_with_retry(
    ctx: &ReleasePhaseContext<'_>,
    release_state: &mut ReleaseState,
    _env_config: &crate::EnvConfig,
) -> Result<()> {
    // Use default retry and timeout configs
    use crate::cli::retry_config::RetryConfig;
    use crate::cli::retry_config::CargoTimeoutConfig;
    let retry_config = RetryConfig::default();
    let timeout_config = CargoTimeoutConfig::default();
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
            || ctx.git_manager.perform_release(ctx.new_version, true),  // Always push
            retry_config.git_operations,
            "Git operations",
            ctx.config,
            None,
        ).await?;
        
        ctx.config.success_println(&format!("✓ Committed: \"{}\"", result.commit.message));
        ctx.config.success_println(&format!("✓ Tagged: {}", result.tag.name));
        ctx.config.success_println("✓ Pushed to origin");
        
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
            retry_config.github_api,
            "GitHub release creation",
            ctx.config,
            None,
        ).await?;
        
        ctx.config.success_println(&format!("✓ Created draft release: {}", release_result.html_url));
        
        // Track release in state for potential cleanup
        release_state.set_github_state(ctx.github_owner.to_string(), ctx.github_repo_name.to_string(), Some(&release_result));
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
    
    // ===== PHASE 4: BUILD RELEASE BINARIES =====
    ctx.config.println("🔨 Building release binaries...");
    
    use tokio::process::Command;
    use tokio::time::{timeout, Duration};
    
    let build_timeout = Duration::from_secs(timeout_config.build_timeout_secs);
    
    // On macOS, build for both architectures to enable universal binaries
    #[cfg(target_os = "macos")]
    let build_targets = vec!["x86_64-apple-darwin", "aarch64-apple-darwin"];

    #[cfg(not(target_os = "macos"))]
    let build_targets: Vec<&str> = vec![];

    if build_targets.is_empty() {
        // Single-target build (non-macOS)
        let build_output = timeout(
            build_timeout,
            Command::new("cargo")
                .arg("build")
                .arg("--release")
                .current_dir(ctx.release_clone_path)
                .output()
        )
        .await
        .map_err(|_| ReleaseError::Cli(CliError::ExecutionFailed {
            command: "cargo build --release".to_string(),
            reason: format!(
                "Build timed out after {} seconds. Try setting KODEGEN_BUILD_TIMEOUT to a higher value.",
                timeout_config.build_timeout_secs
            ),
        }))?
        .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
            command: "cargo build --release".to_string(),
            reason: e.to_string(),
        }))?;

        if !build_output.status.success() {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "cargo build --release".to_string(),
                reason: String::from_utf8_lossy(&build_output.stderr).to_string(),
            }));
        }
    } else {
        // Multi-target build (macOS)
        for target in &build_targets {
            ctx.config.verbose_println(&format!("   Building for {}...", target));

            let build_output = timeout(
                build_timeout,
                Command::new("cargo")
                    .arg("build")
                    .arg("--release")
                    .arg("--target")
                    .arg(target)
                    .current_dir(ctx.release_clone_path)
                    .output()
            )
            .await
            .map_err(|_| ReleaseError::Cli(CliError::ExecutionFailed {
                command: format!("cargo build --release --target {}", target),
                reason: format!(
                    "Build timed out after {} seconds. Try setting KODEGEN_BUILD_TIMEOUT to a higher value.",
                    timeout_config.build_timeout_secs
                ),
            }))?
            .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                command: format!("cargo build --release --target {}", target),
                reason: e.to_string(),
            }))?;

            if !build_output.status.success() {
                return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                    command: format!("cargo build --release --target {}", target),
                    reason: String::from_utf8_lossy(&build_output.stderr).to_string(),
                }));
            }
        }
    }

    ctx.config.success_println("✓ Built release binaries");

    // ===== PHASE 4.5: CREATE UNIVERSAL BINARIES (macOS only) =====
    #[cfg(target_os = "macos")]
    {
        ctx.config.println("🔄 Creating universal binaries (Intel + Apple Silicon)...");

        let workspace_root = ctx.release_clone_path;
        let universal_output = workspace_root.join("target/universal/release");

        // Import the universal binary creator from bundler
        use kodegen_bundler_bundle::bundler::platform::macos::universal::create_universal_binaries;

        match create_universal_binaries(workspace_root, &universal_output) {
            Ok(binaries) => {
                ctx.config.success_println(&format!(
                    "✓ Created {} universal binaries",
                    binaries.len()
                ));
            }
            Err(e) => {
                ctx.config.verbose_println(&format!(
                    "   Warning: Could not create universal binaries: {}",
                    e
                ));
                ctx.config.verbose_println("   Will bundle architecture-specific builds instead");
            }
        }
    }

    // ===== PHASE 5: CREATE PLATFORM BUNDLES =====
    ctx.config.println("📦 Creating platform bundles...");

    // Step 1: Determine which platforms to build
    let all_platforms = get_platforms_to_build();
    ctx.config.verbose_println(&format!("   Building {} platform(s)", all_platforms.len()));

    // Step 2: Separate native vs Docker platforms
    let native_platforms = get_native_platforms(&all_platforms);
    let docker_platforms = get_docker_platforms(&all_platforms);

    ctx.config.verbose_println(&format!(
        "   Native: {} platform(s), Docker: {} platform(s)",
        native_platforms.len(),
        docker_platforms.len()
    ));

    // Step 3: Install bundler and bundle all platforms with incremental upload
    let mut total_artifacts_created = 0;
    let mut total_artifacts_uploaded = 0;
    
    if !native_platforms.is_empty() || !docker_platforms.is_empty() {
        // Ensure bundler is installed from GitHub
        let bundler_binary = ensure_bundler_installed(ctx).await?;

        // Step 4: Bundle and upload native platforms
        for platform in &native_platforms {
            ctx.config.verbose_println(&format!("\n   Building {} (native)...", platform));

            let artifacts = bundle_native_platform(
                ctx,
                &bundler_binary,
                platform,
            ).await?;

            total_artifacts_created += artifacts.len();

            // Upload immediately after bundling
            let uploaded = upload_artifacts_incrementally(
                ctx,
                release_state,
                release_id,
                &artifacts,
                platform,
            ).await?;
            
            total_artifacts_uploaded += uploaded;
        }

        // Step 5: Bundle and upload Docker platforms
        for platform in &docker_platforms {
            ctx.config.verbose_println(&format!("\n   Building {} (Docker)...", platform));

            let artifacts = bundle_docker_platform(
                ctx,
                &bundler_binary,
                platform,
            ).await?;

            total_artifacts_created += artifacts.len();

            // Upload immediately after bundling
            let uploaded = upload_artifacts_incrementally(
                ctx,
                release_state,
                release_id,
                &artifacts,
                platform,
            ).await?;
            
            total_artifacts_uploaded += uploaded;
        }
    }

    // Step 6: Verify artifacts were created
    if total_artifacts_created == 0 {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "bundle".to_string(),
            reason: "No artifacts were created".to_string(),
        }));
    }

    ctx.config.success_println(&format!(
        "✓ Created {} artifact(s) across {} platform(s)",
        total_artifacts_created,
        all_platforms.len()
    ));
    ctx.config.success_println(&format!(
        "✓ Uploaded {} artifact(s) to GitHub release",
        total_artifacts_uploaded
    ));
    
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
            retry_config.release_publishing,
            "Publish GitHub release",
            ctx.config,
            None,
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
    ctx.config.println("📦 Publishing to crates.io...");
    
    // Just use cargo publish directly - simpler and works
    let publish_result = retry_with_backoff(
        || async {
            let output = std::process::Command::new("cargo")
                .arg("publish")
                .current_dir(ctx.release_clone_path)
                .output()
                .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "cargo_publish".to_string(),
                    reason: e.to_string(),
                }))?;
            
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // Check if already published (non-fatal)
                if stderr.contains("already uploaded") {
                    ctx.config.warning_println("⚠️  Package already published to crates.io");
                    return Ok(());
                }
                Err(ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "cargo publish".to_string(),
                    reason: stderr.to_string(),
                }))
            }
        },
        retry_config.release_publishing,
        "Publish to crates.io",
        ctx.config,
        None,
    ).await;
    
    match publish_result {
        Ok(()) => {
            ctx.config.success_println(&format!("✓ Published v{} to crates.io", ctx.new_version));
        }
        Err(e) => {
            ctx.config.warning_println(&format!("⚠️  Publishing failed: {}", e));
            ctx.config.warning_println("   GitHub release is still published successfully");
            // Don't fail the entire release if crates.io publish fails
        }
    }
    
    Ok(())
}

/// Upload artifacts incrementally with state tracking for resume capability
async fn upload_artifacts_incrementally(
    ctx: &ReleasePhaseContext<'_>,
    release_state: &mut ReleaseState,
    release_id: u64,
    artifacts: &[std::path::PathBuf],
    platform: &str,
) -> Result<usize> {
    let mut uploaded_count = 0;
    
    for artifact_path in artifacts {
        let filename = artifact_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "get filename".to_string(),
                    reason: format!("Invalid filename in path: {}", artifact_path.display()),
                })
            })?;
        
        // Check if already uploaded (from state tracking)
        let already_uploaded = release_state
            .github_state
            .as_ref()
            .map(|gh| gh.uploaded_artifacts.contains(&filename.to_string()))
            .unwrap_or(false);
        
        if already_uploaded {
            ctx.config.indent(&format!("⏭ {} (already uploaded)", filename));
            continue;
        }
        
        // Upload this artifact
        ctx.config.indent(&format!("☁️  Uploading {}...", filename));
        
        let uploaded_urls = ctx.github_manager
            .upload_artifacts(
                release_id,
                std::slice::from_ref(artifact_path),
                ctx.new_version,
                ctx.config,
            )
            .await
            .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                command: format!("upload_{}", platform),
                reason: e.to_string(),
            }))?;
        
        if !uploaded_urls.is_empty() {
            // Track uploaded artifact in state
            if let Some(github_state) = &mut release_state.github_state {
                github_state.uploaded_artifacts.push(filename.to_string());
            }
            
            // Save state after each successful upload
            crate::state::save_release_state(release_state).await?;
            
            ctx.config.indent(&format!("✓ Uploaded {}", filename));
            uploaded_count += 1;
        }
    }
    
    Ok(uploaded_count)
}

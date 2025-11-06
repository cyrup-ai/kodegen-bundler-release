//! Release phase execution with retry logic.
//!
//! This module orchestrates phases 2-8 of the release process with intelligent
//! retry logic for network operations.

use crate::error::{CliError, PublishError, ReleaseError, Result};
use crate::publish::{CargoPublisher, PublishConfig};
use crate::state::ReleaseState;
use crate::workspace::{PackageConfig, PackageInfo};

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
    
    // ===== PHASE 4: BUILD RELEASE BINARIES =====
    ctx.config.println("🔨 Building release binaries...");
    
    let build_output = std::process::Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(ctx.temp_dir)
        .output()
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
    
    ctx.config.success_println("✓ Built release binaries");
    
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

    let mut all_artifact_paths: Vec<std::path::PathBuf> = Vec::new();

    // Step 3: Install bundler and bundle all platforms
    if !native_platforms.is_empty() || !docker_platforms.is_empty() {
        // Ensure bundler is installed from GitHub
        let bundler_binary = ensure_bundler_installed(ctx).await?;

        // Step 4: Bundle native platforms
        for platform in &native_platforms {
            ctx.config.verbose_println(&format!("\n   Building {} (native)...", platform));

            let artifacts = bundle_native_platform(
                ctx,
                &bundler_binary,
                platform,
            ).await?;

            all_artifact_paths.extend(artifacts);
        }

        // Step 5: Bundle Docker platforms
        for platform in &docker_platforms {
            ctx.config.verbose_println(&format!("\n   Building {} (Docker)...", platform));

            let artifacts = bundle_docker_platform(
                ctx,
                &bundler_binary,
                platform,
            ).await?;

            all_artifact_paths.extend(artifacts);
        }
    }

    // Step 6: Verify artifacts were created
    if all_artifact_paths.is_empty() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "bundle".to_string(),
            reason: "No artifacts were created".to_string(),
        }));
    }

    ctx.config.success_println(&format!(
        "✓ Created {} artifact(s) across {} platform(s)",
        all_artifact_paths.len(),
        all_platforms.len()
    ));
    
    // ===== PHASE 6: UPLOAD ARTIFACTS TO GITHUB RELEASE =====
    if !all_artifact_paths.is_empty() {
        ctx.config.println("☁️  Uploading artifacts to GitHub release...");
        
        let uploaded_urls = ctx.github_manager
            .upload_artifacts(
                release_id,
                &all_artifact_paths,
                ctx.new_version,
                ctx.config,
            )
            .await
            .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
                command: "upload artifacts".to_string(),
                reason: e.to_string(),
            }))?;
        
        ctx.config.success_println(&format!("✓ Uploaded {} artifacts to GitHub", uploaded_urls.len()));
    } else {
        ctx.config.warning_println("⚠️  No artifacts created - skipping upload");
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
        
        // Create CargoPublisher with configuration from retry_config
        let publisher = CargoPublisher::with_retry_config(
            ctx.config.retry_config.release_publishing as usize,
            std::time::Duration::from_secs(5),  // base delay
            std::time::Duration::from_secs(300), // 5 min timeout
        );
        
        // Build PackageInfo from metadata
        let package_info = PackageInfo {
            name: ctx.metadata.name.clone(),
            version: ctx.new_version.to_string(),
            path: std::path::PathBuf::from("."),
            absolute_path: ctx.temp_dir.to_path_buf(),
            cargo_toml_path: ctx.temp_dir.join("Cargo.toml"),
            config: PackageConfig {
                name: ctx.metadata.name.clone(),
                version: toml::Value::String(ctx.new_version.to_string()),
                edition: None,
                description: None,
                license: None,
                authors: None,
                homepage: None,
                repository: None,
                publish: None,
                other: std::collections::HashMap::new(),
            },
            workspace_dependencies: Vec::new(),
            all_dependencies: std::collections::HashMap::new(),
        };
        
        // Build PublishConfig from options
        let publish_config = PublishConfig {
            registry: ctx.options.registry.clone(),
            dry_run_first: false,
            allow_dirty: false,
            additional_args: vec![],
            token: None, // Uses cargo login credentials
        };
        
        // Publish using CargoPublisher
        match publisher.publish_package(&package_info, &publish_config).await {
            Ok(result) => {
                // Use structured result for better output
                ctx.config.success_println(&result.summary());
                
                // Display warnings if any
                if !result.warnings.is_empty() {
                    for warning in &result.warnings {
                        ctx.config.warning_println(&format!("  ⚠️  {}", warning));
                    }
                }
                
                // Log detailed info in verbose mode
                ctx.config.verbose_println(&format!(
                    "  Published in {:.2}s with {} retry(ies)",
                    result.duration.as_secs_f64(),
                    result.retry_attempts
                ));
            }
            Err(e) => {
                // More informative error handling
                if let ReleaseError::Publish(ref publish_err) = e {
                    match publish_err {
                        PublishError::AlreadyPublished { .. } => {
                            ctx.config.warning_println(&format!("  ⚠️  {}", e));
                        }
                        _ => {
                            ctx.config.verbose_println(&format!(
                                "ℹ️  Skipping crates.io publish: {}",
                                e
                            ));
                        }
                    }
                } else {
                    ctx.config.verbose_println("ℹ️  Skipping crates.io publish (may not be a library crate)");
                }
            }
        }
    }
    
    Ok(())
}

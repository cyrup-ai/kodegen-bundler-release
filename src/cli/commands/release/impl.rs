//! Release implementation logic for isolated execution.
//!
//! This module contains the core release implementation that runs in an isolated
//! temporary clone to prevent modifications to the user's working directory.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, ReleaseError, Result};
use crate::git::{GitConfig, GitManager};
use crate::publish::{Publisher, PublisherConfig};
use crate::state::{ReleaseConfig, ReleasePhase, ReleaseState, create_state_manager_at};
use crate::version::{VersionBump, VersionManager};
use crate::workspace::SharedWorkspaceInfo;
use std::path::PathBuf;
use std::time::Duration;


#[cfg(target_os = "macos")]
use super::super::bundle::build_workspace_binaries_for_target;
#[cfg(not(target_os = "macos"))]
use super::super::bundle::build_workspace_binaries;
use super::super::helpers::{create_bundles, detect_github_repo, parse_github_repo_string};
use super::ReleaseOptions;

/// Perform the actual release logic in the temp directory.
/// This is separated so that cleanup can always happen regardless of success/failure.
pub(super) async fn perform_release_impl(
    temp_dir: &std::path::Path,
    workspace: SharedWorkspaceInfo,
    config: &RuntimeConfig,
    options: &ReleaseOptions,
) -> Result<i32> {
    // Update state file path to use temp directory
    let state_file_path = temp_dir.join(".cyrup_release_state.json");

    // Initialize managers
    let mut version_manager = VersionManager::new(workspace.clone());

    let git_config = GitConfig {
        default_remote: "origin".to_string(),
        annotated_tags: true,
        auto_push_tags: !options.no_push,
        ..Default::default()
    };
    let mut git_manager = GitManager::with_config(&temp_dir, git_config).await?;

    let publisher_config = PublisherConfig {
        inter_package_delay: Duration::from_secs(options.package_delay),
        registry: options.registry.clone(),
        max_concurrent_per_tier: options.concurrent_publishes.unwrap_or(4),
        ..Default::default()
    };
    let mut publisher = Publisher::with_config(workspace.clone(), publisher_config)?;

    // Determine version bump
    let version_bump = VersionBump::try_from(options.bump_type.clone())
        .map_err(|e| ReleaseError::Cli(CliError::InvalidArguments { reason: e }))?;

    // Create release state
    let release_config = ReleaseConfig {
        dry_run_first: true,
        push_to_remote: !options.no_push,
        inter_package_delay_ms: options.package_delay * 1000,
        registry: options.registry.clone(),
        allow_dirty: false,
        ..Default::default()
    };

    let current_version = version_manager.current_version()?;
    let bumper = crate::version::VersionBumper::from_version(current_version.clone());
    let new_version = bumper.bump(version_bump.clone())?;

    let mut release_state =
        ReleaseState::new(new_version.clone(), version_bump.clone(), release_config);

    // Initialize state manager (in temp clone)
    let mut state_manager = create_state_manager_at(&state_file_path)?;

    if options.dry_run {
        config.println("🔍 Performing dry run...");

        // Preview changes
        let preview = version_manager.preview_bump(version_bump.clone())?;
        config.println(&format!("Version preview: {}", preview.format_preview()));

        // Validate packages
        config.println("Validating packages for publishing...");
        // This would call publisher.check_already_published() etc.

        config.success_println("Dry run completed successfully");
        return Ok(0);
    }

    // Begin release process
    config.println(&format!(
        "🚀 Starting release: {} → {}",
        current_version, new_version
    ));

    release_state.add_checkpoint(
        "release_started".to_string(),
        ReleasePhase::Validation,
        None,
        false,
    );
    state_manager.save_state(&release_state).await?;

    // Phase 1: Version Update
    config.println("📝 Updating versions...");
    release_state.set_phase(ReleasePhase::VersionUpdate);
    state_manager.save_state(&release_state).await?;

    let version_result = version_manager.release_version(version_bump)?;

    // Get backups before they're cleared
    let backups = version_manager.updater().get_backups();

    // Store in state
    release_state.set_version_state(&version_result.update_result);

    // Convert TomlBackup to FileBackup for state storage
    if let Some(version_state) = &mut release_state.version_state {
        version_state.backup_files = backups
            .iter()
            .map(|backup| crate::state::FileBackup {
                file_path: backup.file_path.clone(),
                backup_content: backup.content.clone(),
                backup_timestamp: chrono::Utc::now(),
            })
            .collect();
    }

    release_state.add_checkpoint(
        "version_updated".to_string(),
        ReleasePhase::VersionUpdate,
        None,
        true,
    );
    state_manager.save_state(&release_state).await?;

    // Clear backups from version manager now that they're saved in state
    version_manager.updater().clear_backups();

    config.success_println(&format!("Version updated: {}", version_result.summary()));

    // Phase 1.5: Sign Artifacts (macOS only)
    #[cfg(target_os = "macos")]
    let signed_artifacts: Vec<PathBuf> = {
        config.println("🔐 Building and signing macOS artifacts...");

        let sign_dir = temp_dir.join("target/release-artifacts");
        std::fs::create_dir_all(&sign_dir).map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "create_artifact_dir".to_string(),
                reason: e.to_string(),
            })
        })?;

        // Build and sign helper using the sign package
        // CRITICAL: Signing must succeed or build must FAIL
        let helper_zip = kodegen_bundler_sign::build_and_sign_helper(&sign_dir).await.map_err(|e| {
            ReleaseError::Cli(crate::error::CliError::ExecutionFailed {
                command: "build_and_sign_helper".to_string(),
                reason: format!(
                    "FATAL: Code signing failed: {}\n\nUnsigned releases are NEVER allowed!",
                    e
                ),
            })
        })?;

        config.success_println(&format!("✓ Artifact signed: {}", helper_zip.display()));
        vec![helper_zip]
    };
    
    #[cfg(not(target_os = "macos"))]
    let signed_artifacts: Vec<PathBuf> = vec![];

    // Store artifacts in state for potential rollback
    if !signed_artifacts.is_empty() {
        release_state.add_checkpoint(
            "artifacts_signed".to_string(),
            ReleasePhase::VersionUpdate,
            Some(serde_json::Value::String(format!(
                "{} artifact(s) signed",
                signed_artifacts.len()
            ))),
            true,
        );
        state_manager.save_state(&release_state).await?;
    }

    // Phase 1.6: Bundle Artifacts (if requested)
    let bundled_artifacts: Vec<crate::bundler::BundledArtifact> =
        if options.with_bundles || options.upload_bundles {
            // Build binaries for all platforms
            #[cfg(target_os = "macos")]
            {
                // macOS: Always build universal binaries (x86_64 + aarch64)
                config.println("🔨 Building universal binaries (x86_64 + aarch64)...");
                
                // Build for Intel (x86_64)
                config.verbose_println("Building for x86_64-apple-darwin...");
                build_workspace_binaries_for_target(
                    temp_dir,
                    "x86_64-apple-darwin",
                    true,
                    config,
                )?;
                config.verbose_println("✓ Intel (x86_64) binaries built");

                // Build for Apple Silicon (aarch64)
                config.verbose_println("Building for aarch64-apple-darwin...");
                build_workspace_binaries_for_target(
                    temp_dir,
                    "aarch64-apple-darwin",
                    true,
                    config,
                )?;
                config.verbose_println("✓ Apple Silicon (aarch64) binaries built");
                
                // Merge into universal binaries with lipo
                config.verbose_println("Merging architectures with lipo...");
                let universal_dir = temp_dir.join("target/universal/release");
                crate::bundler::platform::macos::universal::create_universal_binaries(
                    temp_dir,
                    &universal_dir,
                )?;
                
                config.success_println("✓ Universal binaries created (x86_64 + aarch64)");
            }
            
            #[cfg(not(target_os = "macos"))]
            {
                // Non-macOS: standard build for current platform
                config.println("🔨 Building release binaries...");
                build_workspace_binaries(temp_dir, true, config)?;
                config.success_println("✓ Build complete");
            }

            config.println("📦 Creating distributable bundles...");
            let artifacts = create_bundles(&workspace, &new_version, config).await?;
            config.success_println(&format!("✓ Created {} bundle(s)", artifacts.len()));
            for artifact in &artifacts {
                config.verbose_println(&format!(
                    "  • {:?} ({} bytes)",
                    artifact.package_type, artifact.size
                ));
            }
            artifacts
        } else {
            vec![]
        };

    // Store bundle info in state if successful
    if !bundled_artifacts.is_empty() {
        release_state.add_checkpoint(
            "artifacts_bundled".to_string(),
            ReleasePhase::VersionUpdate,
            Some(serde_json::Value::String(format!(
                "{} bundle(s) created: {}",
                bundled_artifacts.len(),
                bundled_artifacts
                    .iter()
                    .map(|a| format!("{:?}", a.package_type))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
            true,
        );
        state_manager.save_state(&release_state).await?;
    }

    // Phase 2: Git Operations
    config.println("📦 Creating git commit and tag...");
    release_state.set_phase(ReleasePhase::GitOperations);
    state_manager.save_state(&release_state).await?;

    let git_result = git_manager
        .perform_release(&new_version, !options.no_push)
        .await?;
    release_state.set_git_state(Some(&git_result.commit), Some(&git_result.tag));

    if let Some(push_info) = &git_result.push_info {
        release_state.set_git_push_state(push_info);
    }

    release_state.add_checkpoint(
        "git_operations_complete".to_string(),
        ReleasePhase::GitOperations,
        None,
        true,
    );
    state_manager.save_state(&release_state).await?;

    config.success_println(&format!(
        "Git operations completed: {}",
        git_result.format_result()
    ));

    // Track whether GitHub operations completed with warnings
    let mut github_warnings = false;

    // Phase 2.5: GitHub Release (if enabled)
    if options.github_release {
        config.println("🐙 Creating GitHub release...");
        release_state.set_phase(ReleasePhase::GitHubRelease);
        state_manager.save_state(&release_state).await?;

        // Parse owner/repo from --github-repo or detect from git remote origin
        let owner_repo_result = if let Some(repo_str) = options.github_repo.as_deref() {
            parse_github_repo_string(repo_str)
        } else {
            detect_github_repo(&git_manager).await
        };

        match owner_repo_result {
            Ok((owner, repo)) => {
                // Load release notes if provided
                let release_notes_content = match &options.release_notes {
                    Some(notes_file) => match std::fs::read_to_string(notes_file) {
                        Ok(content) => Some(content),
                        Err(e) => {
                            config.warning_println(&format!(
                                "Failed to read release notes file: {}",
                                e
                            ));
                            None
                        }
                    },
                    None => None,
                };

                let github_config = crate::github::GitHubReleaseConfig {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    draft: options.github_draft,
                    prerelease_for_zero_versions: true,
                    notes: None,
                    token: None, // Will read from GH_TOKEN or GITHUB_TOKEN env var
                };

                match crate::github::GitHubReleaseManager::new(github_config) {
                    Ok(github_manager) => {
                        let commit_sha = &git_result.commit.hash;

                        match github_manager
                            .create_release(&new_version, commit_sha, release_notes_content)
                            .await
                        {
                            Ok(github_result) => {
                                release_state.set_github_state(owner, repo, Some(&github_result));
                                release_state.add_checkpoint(
                                    "github_release_created".to_string(),
                                    ReleasePhase::GitHubRelease,
                                    None,
                                    true,
                                );
                                state_manager.save_state(&release_state).await?;

                                // Show comprehensive success message with timing and status
                                let status_info = format!(
                                    "{}{} ",
                                    if github_result.draft { "[DRAFT] " } else { "" },
                                    if github_result.prerelease {
                                        "[PRERELEASE]"
                                    } else {
                                        ""
                                    }
                                );
                                config.success_println(&format!(
                                    "GitHub release created: {} {}(completed in {:.2}s)",
                                    github_result.html_url,
                                    status_info,
                                    github_result.duration.as_secs_f64()
                                ));

                                // Upload all artifacts (signed + bundled) if any exist
                                let all_artifacts: Vec<PathBuf> = signed_artifacts
                                    .iter()
                                    .chain(bundled_artifacts.iter().flat_map(|ba| ba.paths.iter()))
                                    .filter(|&p| p.is_file()) // Filter out directories (e.g., .app bundles)
                                    .cloned()
                                    .collect();

                                if !all_artifacts.is_empty() {
                                    config.println(&format!(
                                        "📤 Uploading {} artifact(s) to GitHub release...",
                                        all_artifacts.len()
                                    ));

                                    match github_manager
                                        .upload_artifacts(
                                            github_result.release_id,
                                            &all_artifacts,
                                            config,
                                        )
                                        .await
                                    {
                                        Ok(urls) => {
                                            config.success_println(&format!(
                                                "✓ Uploaded {} artifact(s)",
                                                urls.len()
                                            ));

                                            for url in &urls {
                                                config.verbose_println(&format!("  {}", url));
                                            }

                                            // Track successful upload in state
                                            release_state.add_checkpoint(
                                                "artifacts_uploaded".to_string(),
                                                ReleasePhase::GitHubRelease,
                                                Some(serde_json::Value::Object({
                                                    let mut map = serde_json::Map::new();
                                                    map.insert(
                                                        "count".to_string(),
                                                        serde_json::Value::Number(
                                                            urls.len().into(),
                                                        ),
                                                    );
                                                    map.insert(
                                                        "signed".to_string(),
                                                        serde_json::Value::Number(
                                                            signed_artifacts.len().into(),
                                                        ),
                                                    );
                                                    map.insert(
                                                        "bundled".to_string(),
                                                        serde_json::Value::Number(
                                                            bundled_artifacts.len().into(),
                                                        ),
                                                    );
                                                    map
                                                })),
                                                true,
                                            );
                                            state_manager.save_state(&release_state).await?;
                                        }
                                        Err(e) => {
                                            if options.continue_on_github_error {
                                                github_warnings = true;
                                                config.warning_println(&format!(
                                                    "Failed to upload {} artifact(s): {}",
                                                    all_artifacts.len(),
                                                    e
                                                ));
                                                config.warning_println(
                                                    "Continuing due to --continue-on-github-error",
                                                );
                                            } else {
                                                config.error_println(&format!("✗ Failed to upload {} artifact(s) to GitHub release: {}", all_artifacts.len(), e));
                                                config.error_println(&format!(
                                                    "   Release exists at: {}",
                                                    github_result.html_url
                                                ));
                                                return Err(ReleaseError::GitHub(format!(
                                                    "Failed to upload {} artifact(s): {}",
                                                    all_artifacts.len(),
                                                    e
                                                )));
                                            }
                                        }
                                    }
                                }

                                // NEW PHASE 2.6: Remove Draft Status
                                if github_result.draft {
                                    config.println("🚀 Publishing GitHub release (removing draft status)...");
                                    release_state.set_phase(ReleasePhase::GitHubPublish);
                                    state_manager.save_state(&release_state).await?;

                                    match github_manager.publish_draft_release(github_result.release_id).await {
                                        Ok(()) => {
                                            // Update state to reflect release is no longer draft
                                            if let Some(ref mut gh_state) = release_state.github_state {
                                                gh_state.draft = false;
                                            }

                                            release_state.add_checkpoint(
                                                "github_release_published".to_string(),
                                                ReleasePhase::GitHubPublish,
                                                None,
                                                true,
                                            );
                                            state_manager.save_state(&release_state).await?;

                                            config.success_println(&format!(
                                                "✓ GitHub release is now PUBLIC: {}",
                                                github_result.html_url
                                            ));
                                        }
                                        Err(e) => {
                                            if options.continue_on_github_error {
                                                github_warnings = true;
                                                config.warning_println(&format!(
                                                    "Failed to publish GitHub release: {}",
                                                    e
                                                ));
                                                config.warning_println(
                                                    "Continuing with draft release (--continue-on-github-error)",
                                                );
                                            } else {
                                                config.error_println(&format!(
                                                    "✗ Failed to publish GitHub release: {}",
                                                    e
                                                ));
                                                return Err(e);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                if options.continue_on_github_error {
                                    github_warnings = true;
                                    config.warning_println(&format!(
                                        "GitHub release creation failed: {}",
                                        e
                                    ));
                                    config.warning_println(
                                        "Continuing due to --continue-on-github-error",
                                    );
                                } else {
                                    config.error_println(&format!(
                                        "✗ GitHub release creation failed: {}",
                                        e
                                    ));
                                    return Err(ReleaseError::GitHub(e.to_string()));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if options.continue_on_github_error {
                            github_warnings = true;
                            config.warning_println(&format!(
                                "GitHub client initialization failed: {}",
                                e
                            ));
                            config.warning_println("Continuing due to --continue-on-github-error");
                        } else {
                            config.error_println(&format!(
                                "✗ GitHub client initialization failed: {}",
                                e
                            ));
                            return Err(e);
                        }
                    }
                }
            }
            Err(e) => {
                if options.continue_on_github_error {
                    github_warnings = true;
                    config.warning_println(&format!("Failed to detect GitHub repository: {}", e));
                    config.warning_println("Continuing due to --continue-on-github-error");
                } else {
                    config.error_println(&format!("✗ Failed to detect GitHub repository: {}", e));
                    return Err(e);
                }
            }
        }
    }

    // Phase 3: Publishing
    config.println("📤 Publishing packages...");
    release_state.set_phase(ReleasePhase::Publishing);

    let publish_order = crate::workspace::DependencyGraph::build(&workspace)?.publish_order()?;
    release_state.init_publish_state(publish_order.tier_count());
    state_manager.save_state(&release_state).await?;

    let publish_result = publisher.publish_all_packages(config).await?;

    // Update state with publish results
    for package_result in publish_result.successful_publishes.values() {
        release_state.add_published_package(package_result);
    }

    for (package_name, error) in &publish_result.failed_packages {
        release_state.add_failed_package(package_name.clone(), error.clone());
    }

    release_state.add_checkpoint(
        "publishing_complete".to_string(),
        ReleasePhase::Publishing,
        None,
        true,
    );
    state_manager.save_state(&release_state).await?;

    if publish_result.all_successful {
        config.success_println(&format!(
            "Publishing completed: {}",
            publish_result.format_summary()
        ));
    } else {
        config.warning_println(&format!(
            "Publishing partially failed: {}",
            publish_result.format_summary()
        ));
    }

    // Phase 4: Cleanup
    config.println("🧹 Cleaning up...");
    release_state.set_phase(ReleasePhase::Cleanup);
    state_manager.save_state(&release_state).await?;

    // Clear git manager state
    git_manager.clear_release_state();

    // Clear publisher state
    publisher.clear_state();

    // Mark as completed
    release_state.set_phase(ReleasePhase::Completed);
    release_state.add_checkpoint(
        "release_completed".to_string(),
        ReleasePhase::Completed,
        None,
        false,
    );
    state_manager.save_state(&release_state).await?;

    if github_warnings {
        config.warning_println(&format!(
            "⚠️  Release {} completed with GitHub warnings - check output above",
            new_version
        ));
    } else {
        config.success_println(&format!(
            "🎉 Release {} completed successfully!",
            new_version
        ));
    }

    // Cleanup state file after successful completion
    state_manager.cleanup_state()?;

    // Return exit code: 1 for warnings, 0 for complete success
    if github_warnings { Ok(1) } else { Ok(0) }
}

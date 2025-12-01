//! Main release implementation.
//!
//! Coordinates GitHub release + platform bundling.
//! Version bumping and git tagging are handled by `just publish` before this runs.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, ReleaseError, Result};
use crate::state::ReleaseState;
use crate::EnvConfig;

use super::super::super::helpers::parse_github_url;
use super::context::ReleasePhaseContext;
use super::phases::execute_phases_with_retry;

/// Perform release for a repository.
///
/// Expects version already bumped and tagged by `just publish`.
/// This function creates GitHub release and uploads platform bundles.
pub async fn perform_release_single_repo(
    temp_dir: &std::path::Path,
    metadata: crate::metadata::PackageMetadata,
    binary_name: String,
    config: &RuntimeConfig,
    env_config: &EnvConfig,
) -> Result<i32> {
    config
        .println("🚀 Starting GitHub release")
        .expect("Failed to write to stdout");

    // Parse version from metadata (already bumped by `just publish`)
    let release_version = semver::Version::parse(&metadata.version).map_err(|e| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!("Invalid version in Cargo.toml: {}", e),
        })
    })?;

    config
        .println(&format!("   Version: v{}", release_version))
        .expect("Failed to write to stdout");

    // Create release state
    let mut release_state =
        ReleaseState::new(release_version.clone(), crate::state::ReleaseConfig::default());

    // Detect GitHub repository from origin remote
    let origin_url = detect_origin_url(temp_dir).await?;
    let (github_owner, github_repo_name) = parse_github_url(&origin_url)?;

    config
        .verbose_println(&format!(
            "   Repository: {}/{}",
            &github_owner, &github_repo_name
        ))
        .expect("Failed to write to stdout");

    // Initialize GitHub manager
    let github_config = crate::github::GitHubReleaseConfig {
        owner: github_owner.clone(),
        repo: github_repo_name.clone(),
        draft: false,
        prerelease_for_zero_versions: true,
        notes: None,
        token: None, // Will be read from env_config in new()
    };

    let github_manager = crate::github::GitHubReleaseManager::new(github_config, env_config)?;
    config
        .success_println("✓ GitHub API authenticated")
        .expect("Failed to write to stdout");

    // Build context for phase execution
    let ctx = ReleasePhaseContext {
        release_clone_path: temp_dir,
        binary_name: &binary_name,
        new_version: &release_version,
        config,
        github_manager: &github_manager,
        github_owner: &github_owner,
        github_repo_name: &github_repo_name,
    };

    // Execute release phases (GitHub release + bundling)
    execute_phases_with_retry(&ctx, &mut release_state, env_config).await?;

    // Success
    config
        .success_println("🎉 Release complete!")
        .expect("Failed to write to stdout");
    config
        .success_println(&format!("   Package: {}", metadata.name))
        .expect("Failed to write to stdout");
    config
        .success_println(&format!("   Version: v{}", release_version))
        .expect("Failed to write to stdout");

    // Cleanup release state file
    match crate::state::cleanup_release_state(temp_dir) {
        Ok(()) => {
            config
                .verbose_println("✓ Release state cleaned up")
                .expect("Failed to write to stdout");
        }
        Err(e) => {
            config
                .verbose_println(&format!("Warning: Failed to cleanup release state: {}", e))
                .expect("Failed to write to stdout");
        }
    }

    Ok(0)
}

/// Detect origin URL from git config
async fn detect_origin_url(repo_path: &std::path::Path) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "git remote get-url origin".to_string(),
                reason: e.to_string(),
            })
        })?;

    if !output.status.success() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "git remote get-url origin".to_string(),
            reason: String::from_utf8_lossy(&output.stderr).to_string(),
        }));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

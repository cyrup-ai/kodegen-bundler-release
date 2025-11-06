//! GitHub Release management for coordinating release operations

use crate::error::{CliError, ReleaseError, Result};
use bytes::Bytes;
use kodegen_tools_github::{GitHubClient, GitHubReleaseOptions};
use semver::Version;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Configuration for GitHub releases
#[derive(Debug, Clone)]
pub struct GitHubReleaseConfig {
    /// Repository owner
    pub owner: String,
    /// Repository name
    pub repo: String,
    /// Whether to create draft releases
    pub draft: bool,
    /// Whether to mark as pre-release for pre-1.0 versions
    pub prerelease_for_zero_versions: bool,
    /// Custom release notes
    pub notes: Option<String>,
    /// GitHub token (from environment or config)
    pub token: Option<String>,
}

impl Default for GitHubReleaseConfig {
    fn default() -> Self {
        Self {
            owner: String::new(),
            repo: String::new(),
            draft: false,
            prerelease_for_zero_versions: true,
            notes: None,
            token: None,
        }
    }
}

/// Result of GitHub release operation
#[derive(Debug, Clone)]
pub struct GitHubReleaseResult {
    /// Release ID
    pub release_id: u64,
    /// Release URL
    pub html_url: String,
    /// Whether this was a draft
    pub draft: bool,
    /// Whether this was a prerelease
    pub prerelease: bool,
}

/// GitHub release manager
pub struct GitHubReleaseManager {
    /// GitHub client
    client: GitHubClient,
    /// Configuration
    config: GitHubReleaseConfig,
}

/// One-time initialization guard for rustls crypto provider
/// 
/// Ensures install_default() is called exactly once per process, even when
/// GitHubReleaseManager::new() is called multiple times. This follows rustls
/// official best practices for provider initialization.
static RUSTLS_INITIALIZED: OnceLock<()> = OnceLock::new();

impl GitHubReleaseManager {
    /// Create new GitHub release manager
    pub fn new(config: GitHubReleaseConfig, env_config: &crate::EnvConfig) -> Result<Self> {
        // Initialize rustls crypto provider exactly once per process
        // Uses OnceLock to ensure install_default() succeeds on first call only
        RUSTLS_INITIALIZED.get_or_init(|| {
            rustls::crypto::ring::default_provider()
                .install_default()
                .unwrap_or_else(|e| {
                    panic!("FATAL: Failed to install rustls crypto provider: {:?}. \
                           This indicates another crypto provider may already be installed, \
                           or the system is in an invalid state.", e)
                })
        });
        
        // Get token from config or environment
        let token = config.token.clone()
            .or_else(|| env_config.get("GH_TOKEN"))
            .or_else(|| env_config.get("GITHUB_TOKEN"))
            .ok_or_else(|| ReleaseError::Cli(CliError::InvalidArguments {
                reason: "GitHub token not provided. Set GH_TOKEN or GITHUB_TOKEN environment variable or use --github-token".to_string(),
            }))?;

        let client = GitHubClient::with_token(token).map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "github_client_init".to_string(),
                reason: e.to_string(),
            })
        })?;

        Ok(Self { client, config })
    }

    /// Create a GitHub release
    pub async fn create_release(
        &self,
        version: &Version,
        commit_sha: &str,
        release_notes: Option<String>,
    ) -> Result<GitHubReleaseResult> {
        let tag_name = format!("v{}", version);

        // Determine if this should be a prerelease
        let is_prerelease = if self.config.prerelease_for_zero_versions {
            version.major == 0 || !version.pre.is_empty()
        } else {
            !version.pre.is_empty()
        };

        // Use provided release notes or custom notes from config
        let body = release_notes
            .or_else(|| self.config.notes.clone())
            .or_else(|| Some(format!("Release version {}", version)));

        let options = GitHubReleaseOptions {
            tag_name: tag_name.clone(),
            target_commitish: Some(commit_sha.to_string()),
            name: Some(format!("Release {}", version)),
            body,
            draft: self.config.draft,
            prerelease: is_prerelease,
        };

        let result = kodegen_tools_github::create_release(
            self.client.inner().clone(),
            &self.config.owner,
            &self.config.repo,
            options,
        )
        .await
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "create_github_release".to_string(),
                reason: e.to_string(),
            })
        })?;

        Ok(GitHubReleaseResult {
            release_id: result.id,
            html_url: result.html_url,
            draft: result.draft,
            prerelease: result.prerelease,
        })
    }

    /// Delete a release (for rollback)
    pub async fn delete_release(&self, release_id: u64) -> Result<()> {
        kodegen_tools_github::delete_release(
            self.client.inner().clone(),
            &self.config.owner,
            &self.config.repo,
            release_id,
        )
        .await
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "delete_github_release".to_string(),
                reason: e.to_string(),
            })
        })
    }

    /// Check if a release already exists for this version
    ///
    /// Uses the GitHub API to check if a release exists with tag v{version}.
    ///
    /// # Returns
    /// - `Ok(true)` - Release exists
    /// - `Ok(false)` - Release does not exist  
    /// - `Err(_)` - Network or authentication error
    pub async fn release_exists(&self, version: &Version) -> Result<bool> {
        let tag_name = format!("v{}", version);
        
        match kodegen_tools_github::get_release_by_tag(
            self.client.inner().clone(),
            &self.config.owner,
            &self.config.repo,
            &tag_name,
        )
        .await
        {
            Ok(Some(_)) => Ok(true),  // Release exists
            Ok(None) => Ok(false), // Doesn't exist
            Err(e) => Err(ReleaseError::GitHub(e.to_string())), // Network/auth error
        }
    }

    /// Clean up existing GitHub release for this version
    ///
    /// Finds and deletes the GitHub release with tag v{version} if it exists.
    /// Safe to call even if release doesn't exist - will silently succeed.
    ///
    /// # Returns
    /// - `Ok(())` - Release deleted or didn't exist
    /// - `Err(_)` - Network or authentication error
    pub async fn cleanup_existing_release(&self, version: &Version) -> Result<()> {
        let tag_name = format!("v{}", version);
        
        // Get release by tag to find the release_id
        match kodegen_tools_github::get_release_by_tag(
            self.client.inner().clone(),
            &self.config.owner,
            &self.config.repo,
            &tag_name,
        )
        .await
        {
            Ok(Some(release)) => {
                // Release exists - delete it
                self.delete_release(release.id.0).await?;
                Ok(())
            }
            Ok(None) => {
                // Release doesn't exist - nothing to cleanup
                Ok(())
            }
            Err(e) => Err(ReleaseError::GitHub(e.to_string())),
        }
    }

    /// Publish a draft release (remove draft status)
    ///
    /// Converts a draft release to a published release by setting draft=false.
    /// This makes the release visible to all users.
    ///
    /// # Arguments
    /// * `release_id` - GitHub release ID from create_release result
    ///
    /// # Returns
    /// * `Ok(())` - Release is now public
    /// * `Err` - Failed to update release
    pub async fn publish_draft_release(&self, release_id: u64) -> Result<()> {
        kodegen_tools_github::update_release(
            self.client.inner().clone(),
            &self.config.owner,
            &self.config.repo,
            release_id,
            Some(false),
        )
        .await
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "publish_github_release".to_string(),
                reason: e.to_string(),
            })
        })?;

        Ok(())
    }

    /// Test GitHub API connection and authentication
    ///
    /// Calls /user endpoint to verify the token is valid before attempting
    /// expensive operations like creating releases or uploading artifacts.
    ///
    /// This is a PRECHECK - called before Phase 3 to fail fast if authentication
    /// is broken. Prevents wasting time on git operations when GitHub access will fail.
    ///
    /// # Returns
    /// - `Ok(true)` - API connection successful, token is valid
    /// - `Ok(false)` - API connection failed (invalid token, network error, or rate limit)
    pub async fn test_connection(&self) -> Result<bool> {
        match self.client.get_me().await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Verify a release exists and is still a draft
    ///
    /// Used before Phase 7 to ensure the release wasn't accidentally published
    /// or deleted between Phase 3 (creation) and Phase 7 (publish).
    ///
    /// # Arguments
    /// * `release_id` - GitHub release ID from create_release result
    ///
    /// # Returns
    /// - `Ok(true)` - Release exists and is still a draft
    /// - `Ok(false)` - Release doesn't exist, is already published, or network error
    pub async fn verify_release_is_draft(&self, release_id: u64) -> Result<bool> {
        match self.client
            .inner()
            .repos(&self.config.owner, &self.config.repo)
            .releases()
            .get(release_id)
            .await
        {
            Ok(release) => Ok(release.draft),
            Err(_) => Ok(false),
        }
    }

    /// Get list of assets already uploaded to a release
    ///
    /// Returns a HashSet of asset filenames for fast lookup.
    ///
    /// Uses octocrab::models::repos::Release which includes:
    /// - `assets: Vec<octocrab::models::repos::Asset>` - List of uploaded assets
    /// - Each Asset has `name: String` field for filename comparison
    pub async fn get_release_asset_names(
        &self,
        version: &semver::Version,
    ) -> Result<std::collections::HashSet<String>> {
        use kodegen_tools_github::get_release_by_tag;

        let tag_name = format!("v{}", version);

        let release = get_release_by_tag(
            self.client.inner().clone(),
            &self.config.owner,
            &self.config.repo,
            &tag_name,
        )
        .await
        .map_err(|e| ReleaseError::GitHub(e.to_string()))?;

        // If release doesn't exist, return empty set
        let release = match release {
            Some(r) => r,
            None => return Ok(std::collections::HashSet::new()),
        };

        // Extract asset names from octocrab Release.assets Vec
        let asset_names: std::collections::HashSet<String> = release
            .assets
            .iter()
            .map(|asset| asset.name.clone())
            .collect();

        Ok(asset_names)
    }

    /// Upload signed artifacts to release
    ///
    /// Reads artifact files and uploads them as release assets.
    /// Returns list of download URLs for the uploaded assets.
    pub async fn upload_artifacts(
        &self,
        release_id: u64,
        artifact_paths: &[PathBuf],
        version: &semver::Version,
        runtime_config: &crate::cli::RuntimeConfig,
    ) -> Result<Vec<String>> {
        let mut uploaded_urls = Vec::new();

        // Query existing assets ONCE before upload loop
        runtime_config.verbose_println("   Checking for existing assets...");
        let existing_assets = self.get_release_asset_names(version).await?;

        if !existing_assets.is_empty() {
            runtime_config.verbose_println(&format!(
                "   Found {} existing asset(s)",
                existing_assets.len()
            ));
        }

        for artifact_path in artifact_paths {
            // Safety check: should be filtered at call site, but double-check
            if !artifact_path.is_file() {
                runtime_config.warning_println(&format!(
                    "⚠️  Skipping non-file artifact: {}",
                    artifact_path.display()
                ));
                continue;
            }

            // Extract filename for the asset
            let filename = artifact_path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    ReleaseError::Cli(CliError::InvalidArguments {
                        reason: format!("Invalid artifact filename: {:?}", artifact_path),
                    })
                })?;

            // IDEMPOTENCY: Skip if already uploaded
            if existing_assets.contains(filename) {
                runtime_config.indent(&format!("✓ Skipping {} (already uploaded)", filename));
                continue;
            }

            // Read file content
            let content = std::fs::read(artifact_path).map_err(|e| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "read_artifact".to_string(),
                    reason: e.to_string(),
                })
            })?;

            // Create upload options
            let upload_options = kodegen_tools_github::UploadAssetOptions {
                release_id,
                asset_name: filename.to_string(),
                label: Some(create_artifact_label(filename)),
                content: Bytes::from(content),
                replace_existing: false, // Safer default - fails if asset exists
            };

            // Upload via GitHub client
            let asset = self
                .client
                .upload_release_asset(&self.config.owner, &self.config.repo, upload_options)
                .await
                .map_err(|e| ReleaseError::GitHub(e.to_string()))?;

            // Extract download URL from asset
            uploaded_urls.push(asset.browser_download_url.to_string());

            runtime_config.indent(&format!("✓ Uploaded: {} ({} bytes)", filename, asset.size));
        }

        Ok(uploaded_urls)
    }
}

/// Detect MIME type for bundle artifacts
///
/// Note: octocrab automatically detects content types from file extensions,
/// but we provide this for future extensibility and explicit documentation.
#[allow(dead_code)]
fn detect_bundle_content_type(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("deb") => "application/vnd.debian.binary-package",
        Some("rpm") => "application/x-rpm",
        Some("exe") => "application/x-msdownload",
        Some("dmg") => "application/x-apple-diskimage",
        Some("AppImage") => "application/x-executable",
        Some("zip") => "application/zip",
        Some("tar") | Some("gz") | Some("tgz") => "application/gzip",
        _ => "application/octet-stream",
    }
}

/// Create descriptive label for artifact based on filename
fn create_artifact_label(filename: &str) -> String {
    // Extract architecture
    let arch = if filename.contains("aarch64") || filename.contains("arm64") {
        "ARM64"
    } else if filename.contains("x86_64") || filename.contains("amd64") {
        "x86_64"
    } else {
        "multi-arch"
    };

    // Extract platform
    let platform = if filename.contains("deb") {
        "Debian/Ubuntu"
    } else if filename.contains("rpm") {
        "RedHat/Fedora"
    } else if filename.contains("dmg") {
        "macOS"
    } else if filename.contains(".exe") {
        "Windows"
    } else if filename.contains("AppImage") {
        "Linux AppImage"
    } else {
        "Binary"
    };

    format!("kodegen {} - {}", platform, arch)
}

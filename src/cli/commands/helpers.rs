//! Shared helper functions for command execution.

use crate::error::{CliError, ReleaseError, Result};
use crate::git::GitManager;
use crate::workspace::WorkspaceInfo;

/// Parse GitHub repository string into owner/repo tuple
#[allow(dead_code)]
pub(super) fn parse_github_repo(repo_str: Option<&str>) -> Result<(String, String)> {
    let repo = repo_str.ok_or_else(|| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason: "--github-repo is required when --github-release is used. Format: owner/repo"
                .to_string(),
        })
    })?;

    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Invalid GitHub repository format: '{}'. Expected: owner/repo",
                repo
            ),
        }));
    }

    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Parse GitHub repo string "owner/repo"
pub(super) fn parse_github_repo_string(repo_str: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = repo_str.split('/').collect();
    if parts.len() != 2 {
        return Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Invalid GitHub repository format: '{}'. Expected: owner/repo",
                repo_str
            ),
        }));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Parse GitHub owner/repo from git remote URL
/// Supports: git@github.com:owner/repo.git and https://github.com/owner/repo.git
pub(super) fn parse_github_url(url: &str) -> Option<(String, String)> {
    // Handle git@github.com:owner/repo.git (with or without leading slash)
    if let Some(ssh_part) = url.strip_prefix("git@github.com:") {
        // Remove leading slash if present (malformed URL like git@github.com:/owner/repo)
        let ssh_part = ssh_part.strip_prefix('/').unwrap_or(ssh_part);
        let repo_part = ssh_part.strip_suffix(".git").unwrap_or(ssh_part);
        let parts: Vec<&str> = repo_part.split('/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // Handle https://github.com/owner/repo.git
    if url.contains("github.com/")
        && let Some(path) = url.split("github.com/").nth(1)
    {
        let repo_part = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<&str> = repo_part.split('/').collect();
        if parts.len() >= 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    None
}

/// Detect GitHub repo from git remote origin using GitManager
pub(super) async fn detect_github_repo(git_manager: &GitManager) -> Result<(String, String)> {
    let remotes = git_manager.remotes().await?;

    // Find origin remote
    let origin = remotes.iter().find(|r| r.name == "origin").ok_or_else(|| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason:
                "No 'origin' remote configured. Git requires origin for push/pull/tag operations."
                    .to_string(),
        })
    })?;

    // Parse GitHub URL from origin
    parse_github_url(&origin.fetch_url).ok_or_else(|| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Origin remote is not a GitHub repository: {}",
                origin.fetch_url
            ),
        })
    })
}

/// Create distributable bundles for the release
pub(super) async fn create_bundles(
    workspace: &WorkspaceInfo,
    version: &semver::Version,
    _config: &crate::cli::RuntimeConfig,
) -> Result<Vec<crate::bundler::BundledArtifact>> {
    use crate::bundler::{BundleSettings, Bundler, PackageSettings, SettingsBuilder};
    use std::path::PathBuf;

    // Use brand name "Kodegen" as product name for user-facing installer
    // (matches assets/img/kodegen.icns and creates Kodegen.app/Kodegen-{version}.dmg)
    let product_name = "Kodegen".to_string();

    // Extract description from workspace config
    let description = workspace
        .workspace_config
        .package
        .as_ref()
        .and_then(|p| p.other.get("description"))
        .and_then(|d| d.as_str())
        .unwrap_or("Rust application")
        .to_string();

    // Build package settings with workspace metadata
    let package_settings = PackageSettings {
        product_name,
        version: version.to_string(),
        description,
        ..Default::default()
    };

    // Configure icon paths from assets directory (multi-resolution)
    let icon_paths = vec![
        workspace.root.join("assets/img/icon_16x16.png"),
        workspace.root.join("assets/img/icon_16x16@2x.png"),
        workspace.root.join("assets/img/icon_32x32.png"),
        workspace.root.join("assets/img/icon_32x32@2x.png"),
        workspace.root.join("assets/img/icon_128x128.png"),
        workspace.root.join("assets/img/icon_128x128@2x.png"),
        workspace.root.join("assets/img/icon_256x256.png"),
        workspace.root.join("assets/img/icon_256x256@2x.png"),
        workspace.root.join("assets/img/icon_512x512.png"),
        workspace.root.join("assets/img/icon_512x512@2x.png"),
    ];

    // Configure bundle settings with icons and post-install scripts
    // Note: macOS .app bundles don't support post-install scripts - kodegen_install
    // must run automatically on first app launch instead
    use crate::bundler::{DebianSettings, RpmSettings};
    let bundle_settings = BundleSettings {
        identifier: Some(format!("ai.kodegen.{}", package_settings.product_name)),
        icon: Some(icon_paths),
        deb: DebianSettings {
            post_install_script: Some(PathBuf::from("packages/bundler-release/postinst.deb.sh")),
            ..Default::default()
        },
        rpm: RpmSettings {
            post_install_script: Some(PathBuf::from("packages/bundler-release/postinst.rpm.sh")),
            ..Default::default()
        },
        ..Default::default()
    };

    // Configure all required binaries for the bundle (18 total)
    // kodegen_install runs first to setup system and register kodegend daemon
    use crate::bundler::BundleBinary;
    let binaries = vec![
        BundleBinary::new("kodegen_install".to_string(), true), // primary installer (runs first)
        BundleBinary::new("kodegend".to_string(), false),       // service daemon
        BundleBinary::new("kodegen".to_string(), false),        // main stdio MCP server
        // HTTP category servers (15 binaries) - daemon launches these on ports 30438-30452
        BundleBinary::new("kodegen-browser".to_string(), false),
        BundleBinary::new("kodegen-citescrape".to_string(), false),
        BundleBinary::new("kodegen-claude-agent".to_string(), false),
        BundleBinary::new("kodegen-config".to_string(), false),
        BundleBinary::new("kodegen-database".to_string(), false),
        BundleBinary::new("kodegen-filesystem".to_string(), false),
        BundleBinary::new("kodegen-git".to_string(), false),
        BundleBinary::new("kodegen-github".to_string(), false),
        BundleBinary::new("kodegen-introspection".to_string(), false),
        BundleBinary::new("kodegen-process".to_string(), false),
        BundleBinary::new("kodegen-prompt".to_string(), false),
        BundleBinary::new("kodegen-reasoner".to_string(), false),
        BundleBinary::new("kodegen-sequential-thinking".to_string(), false),
        BundleBinary::new("kodegen-terminal".to_string(), false),
        BundleBinary::new("kodegen-candle-agent".to_string(), false),
    ];

    // Determine output directory - check for universal binaries first (macOS)
    let universal_dir = workspace.root.join("target/universal/release");
    let out_dir = if universal_dir.exists() {
        universal_dir
    } else {
        workspace.root.join("target/release")
    };

    // Use SettingsBuilder to create Settings
    let settings = SettingsBuilder::new()
        .project_out_directory(out_dir)
        .package_settings(package_settings)
        .bundle_settings(bundle_settings)
        .binaries(binaries)
        .build()
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "build_settings".to_string(),
                reason: e.to_string(),
            })
        })?;

    // Now Bundler::new() gets the correct Settings type
    let bundler = Bundler::new(settings).await.map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "create_bundler".to_string(),
            reason: e.to_string(),
        })
    })?;

    // Build ALL platforms (macOS, Linux, Windows) using Docker for cross-platform
    use crate::bundler::PackageType;
    let all_platforms = vec![
        PackageType::MacOsBundle,
        PackageType::Dmg,
        PackageType::Deb,
        PackageType::Rpm,
        PackageType::AppImage,
        PackageType::Nsis,
    ];

    // Split platforms by execution environment (native vs container)
    let (native_platforms, container_platforms) =
        crate::cli::docker::split_platforms_by_host(&all_platforms);

    let mut all_artifacts = Vec::new();

    // Build native platforms locally
    if !native_platforms.is_empty() {
        _config.println(&format!("📦 Bundling {} native platform(s)...", native_platforms.len()));
        let artifacts = bundler.bundle_types(&native_platforms).await.map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "bundler_execute_native".to_string(),
                reason: e.to_string(),
            })
        })?;
        all_artifacts.extend(artifacts);
    }

    // Build containerized platforms in Docker
    if !container_platforms.is_empty() {
        _config.println(&format!("📦 Bundling {} cross-platform package(s) in container...", container_platforms.len()));

        // Check Docker availability
        crate::cli::docker::check_docker_available().await?;

        // Ensure builder image exists (never rebuild during release for consistency)
        crate::cli::docker::ensure_image_built(&workspace.root, false, _config).await?;

        // Create container bundler with default resource limits
        let container = crate::cli::docker::ContainerBundler::with_limits(
            workspace.root.clone(),
            crate::cli::docker::ContainerLimits::default(),
        );

        // Bundle each platform in container
        for platform in container_platforms {
            _config.verbose_println(&format!("  Building {:?}...", platform));
            let paths = container
                .bundle_platform(platform, false, true, _config) // no_build=false, release=true
                .await?;

            // Convert paths to BundledArtifact
            let size = paths.iter().fold(0u64, |acc, p| {
                acc + std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
            });

            let checksum = if !paths.is_empty() {
                calculate_checksum(&paths[0])?
            } else {
                String::new()
            };

            all_artifacts.push(crate::bundler::BundledArtifact {
                package_type: platform,
                paths,
                size,
                checksum,
            });
        }
    }

    Ok(all_artifacts)
}

/// Prompt user for confirmation with y/n input
/// 
/// Returns true if user confirms (y/yes), false if user declines (n/no/empty)
/// 
/// # Arguments
/// * `prompt` - The question to ask (without [y/N] suffix)
/// 
/// # Example
/// ```
/// if !prompt_confirmation("About to delete files")? {
///     println!("Operation cancelled");
///     return Ok(());
/// }
/// ```
pub(super) fn prompt_confirmation(prompt: &str) -> std::io::Result<bool> {
    use std::io::Write;
    
    print!("{} [y/N]: ", prompt);
    std::io::stdout().flush()?;
    
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    
    let response = input.trim().to_lowercase();
    Ok(matches!(response.as_str(), "y" | "yes"))
}

/// Calculate SHA-256 checksum of a file
fn calculate_checksum(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "read_file_for_checksum".to_string(),
            reason: e.to_string(),
        })
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];

    loop {
        let n = file.read(&mut buffer).map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "read_file_for_checksum".to_string(),
                reason: e.to_string(),
            })
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

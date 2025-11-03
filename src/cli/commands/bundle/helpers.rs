//! Bundle helper functions.
//!
//! Shared utilities for bundle creation and GitHub upload.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, ReleaseError, Result};
use crate::git::GitManager;

// TOML parsing for Cargo.toml binary discovery
use toml;

/// Default product name for bundles
const DEFAULT_PRODUCT_NAME: &str = "kodegen";

/// Default product description for bundles
const DEFAULT_PRODUCT_DESCRIPTION: &str =
    "KODEGEN.ᴀɪ: Memory-efficient, Blazing-Fast, MCP tools for code generation agents.";

/// Discover binaries from workspace members by parsing Cargo.toml
pub(super) fn discover_binaries_from_workspace(
    workspace: &crate::workspace::WorkspaceInfo,
) -> Result<Vec<crate::bundler::BundleBinary>> {
    // Use filter_map to read each Cargo.toml only once
    let binaries = workspace
        .packages
        .values()
        .filter_map(|pkg| {
            // Read Cargo.toml to extract binary name
            let manifest = std::fs::read_to_string(&pkg.cargo_toml_path).ok()?;

            // Parse TOML to find [[bin]] section and extract name
            let toml_value = toml::from_str::<toml::Value>(&manifest).ok()?;

            // Get [[bin]] array
            let bin_array = toml_value.get("bin").and_then(|v| v.as_array())?;

            // Get first binary name from [[bin]] array
            let bin_name = bin_array
                .first()
                .and_then(|b| b.get("name"))
                .and_then(|n| n.as_str())?;

            // EXCLUDE BUILD-TIME UTILITIES (kodegen_bundler_* packages)
            // These are development/build tools, not runtime binaries for distribution
            if bin_name.starts_with("kodegen_bundler_") {
                return None;
            }

            // kodegen_install is the main executable (installer launcher)
            let is_main = bin_name == "kodegen_install";
            Some(crate::bundler::BundleBinary::new(
                bin_name.to_string(),
                is_main,
            ))
        })
        .collect::<Vec<_>>();

    // Verify kodegen_install was found and marked as main
    if !binaries.iter().any(|b| b.main()) {
        return Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: "kodegen_install binary not found in workspace. Required for bundling."
                .to_string(),
        }));
    }

    Ok(binaries)
}

/// Build workspace binaries with cargo
pub(crate) fn build_workspace_binaries(
    workspace_path: &std::path::Path,
    release: bool,
    config: &RuntimeConfig,
) -> Result<()> {
    use std::process::Command;

    // All binaries for kodegen release - includes installer, daemon, and all MCP servers
    let required_binaries = [
        // Core binaries
        "kodegen_install",  // Installer
        "kodegen",          // Main MCP server (stdio)
        "kodegend",         // Daemon
        // MCP Server Tools
        "kodegen-browser",
        "kodegen-candle-agent",
        "kodegen-citescrape",
        "kodegen-claude-agent",
        "kodegen-config",
        "kodegen-database",
        "kodegen-filesystem",
        "kodegen-git",
        "kodegen-github",
        "kodegen-introspection",
        "kodegen-process",
        "kodegen-prompt",
        "kodegen-reasoner",
        "kodegen-sequential-thinking",
        "kodegen-terminal",
    ];

    // Build all binaries in parallel using cargo's built-in parallelization
    config.verbose_println(&format!(
        "Building {} binaries in parallel{}",
        required_binaries.len(),
        if release { " (release mode)" } else { "" }
    ));

    // Package names for the 18 required binaries
    let required_packages = [
        "kodegen_bundler_install",
        "kodegen",
        "kodegen_daemon",
        "kodegen_tools_browser",
        "kodegen_candle_agent",
        "kodegen_tools_citescrape",
        "kodegen_claude_agent",
        "kodegen_tools_config",
        "kodegen_tools_database",
        "kodegen_tools_filesystem",
        "kodegen_tools_git",
        "kodegen_tools_github",
        "kodegen_tools_introspection",
        "kodegen_tools_process",
        "kodegen_tools_prompt",
        "kodegen_tools_reasoner",
        "kodegen_tools_sequential_thinking",
        "kodegen_tools_terminal",
    ];

    // Build all packages in a single cargo invocation
    // This ensures cargo can properly resolve and share dependencies across all packages
    config.verbose_println(&format!(
        "Building {} packages...",
        required_packages.len()
    ));

    let mut cmd = Command::new("cargo");
    cmd.current_dir(workspace_path);
    cmd.arg("build");

    // Add each required binary explicitly
    for binary in &required_binaries {
        cmd.arg("--bin");
        cmd.arg(binary);
    }

    if release {
        cmd.arg("--release");
    }

    // Limit parallelism to prevent OOM in Docker with LTO-enabled dependencies
    // Heavy crates like surrealdb use linker-plugin-lto which can consume 4-8GB per process
    cmd.arg("-j");
    cmd.arg("2");

    let output = cmd.output().map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "cargo_build_all".to_string(),
            reason: e.to_string(),
        })
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "cargo_build_all".to_string(),
            reason: format!("Failed to build packages:\n{}", stderr),
        }));
    }

    // Verify all required binaries were built
    let target_dir = if release { "release" } else { "debug" };
    for binary in &required_binaries {
        let bin_path = workspace_path.join(format!("target/{}/{}", target_dir, binary));
        if !bin_path.exists() {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "verify_binaries".to_string(),
                reason: format!("Required binary not found after build: {}", binary),
            }));
        }
    }

    config.verbose_println(&format!("✓ Verified all {} binaries exist", required_binaries.len()));

    Ok(())
}

/// Build workspace binaries for a specific target architecture
///
/// This is used for cross-architecture builds (e.g., building x86_64 on Apple Silicon).
///
/// # Arguments
/// * `workspace_path` - Root of workspace
/// * `target` - Target triple (e.g., "x86_64-apple-darwin", "aarch64-apple-darwin")
/// * `release` - Whether to build in release mode
///
/// # Binaries Built
/// See ALL_BINARIES list - builds all 18 binaries (3 core + 15 category servers)
pub(crate) fn build_workspace_binaries_for_target(
    workspace_path: &std::path::Path,
    target: &str,
    release: bool,
    config: &RuntimeConfig,
) -> Result<()> {
    use std::process::Command;

    // Same binary list as build_workspace_binaries() (helpers.rs:72-92)
    let required_binaries = [
        // Core binaries
        "kodegen_install",
        "kodegen",
        "kodegend",
        // Category servers
        "kodegen-browser",
        "kodegen-candle-agent",
        "kodegen-citescrape",
        "kodegen-claude-agent",
        "kodegen-config",
        "kodegen-database",
        "kodegen-filesystem",
        "kodegen-git",
        "kodegen-github",
        "kodegen-introspection",
        "kodegen-process",
        "kodegen-prompt",
        "kodegen-reasoner",
        "kodegen-sequential-thinking",
        "kodegen-terminal",
    ];

    // Build all binaries in parallel for this target
    config.verbose_println(&format!(
        "Building {} binaries for target {}{}",
        required_binaries.len(),
        target,
        if release { " (release mode)" } else { "" }
    ));

    // Package names for the 18 required binaries
    let required_packages = [
        "kodegen_bundler_install",
        "kodegen",
        "kodegen_daemon",
        "kodegen_tools_browser",
        "kodegen_candle_agent",
        "kodegen_tools_citescrape",
        "kodegen_claude_agent",
        "kodegen_tools_config",
        "kodegen_tools_database",
        "kodegen_tools_filesystem",
        "kodegen_tools_git",
        "kodegen_tools_github",
        "kodegen_tools_introspection",
        "kodegen_tools_process",
        "kodegen_tools_prompt",
        "kodegen_tools_reasoner",
        "kodegen_tools_sequential_thinking",
        "kodegen_tools_terminal",
    ];

    let mut cmd = Command::new("cargo");
    cmd.current_dir(workspace_path);
    cmd.arg("build");

    // Add each package with -p flag (cargo parallelizes these automatically)
    for package in &required_packages {
        cmd.arg("-p");
        cmd.arg(package);
    }

    cmd.arg("--target");
    cmd.arg(target);

    if release {
        cmd.arg("--release");
    }

    let output = cmd.output().map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("cargo_build_bins_{}", target),
            reason: e.to_string(),
        })
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("cargo_build_bins_{}", target),
            reason: format!("Failed to build binaries for {}:\n{}", target, stderr),
        }));
    }

    // Verify all required binaries were built
    let target_dir = if release { "release" } else { "debug" };
    for binary in &required_binaries {
        let bin_path = workspace_path.join(format!("target/{}/{}/{}", target, target_dir, binary));
        if !bin_path.exists() {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "verify_binaries".to_string(),
                reason: format!("Required binary not found after build for {}: {}", target, binary),
            }));
        }
    }

    config.verbose_println(&format!("✓ Verified all {} binaries exist for {}", required_binaries.len(), target));

    Ok(())
}

/// Create bundler settings from workspace analysis
pub(super) fn create_bundler_settings(
    workspace: &crate::workspace::WorkspaceInfo,
    binaries: &[crate::bundler::BundleBinary],
    override_name: &Option<String>,
    override_version: &Option<String>,
    release: bool,
    target_override: &Option<String>,
    platform: Option<&str>,
) -> Result<crate::bundler::Settings> {
    use crate::bundler::{PackageSettings, SettingsBuilder};

    // Use product-level metadata (deterministic and semantically correct)
    let name = DEFAULT_PRODUCT_NAME.to_string();
    let description = DEFAULT_PRODUCT_DESCRIPTION.to_string();

    // Get version from workspace configuration
    let version = workspace
        .workspace_config
        .package
        .as_ref()
        .and_then(|p| p.version.clone())
        .unwrap_or_else(|| "0.0.0".to_string());

    // Determine output directory
    // Check for universal binaries first (macOS universal builds)
    let universal_dir = std::path::PathBuf::from("target/universal/release");
    let out_dir = if universal_dir.exists() {
        // Use universal binaries if they exist
        universal_dir
    } else if release {
        std::path::PathBuf::from("target/release")
    } else {
        std::path::PathBuf::from("target/debug")
    };

    // Determine target triple
    let target = target_override.clone().unwrap_or_else(|| {
        std::env::var("TARGET").unwrap_or_else(|_| {
            // Detect from current platform
            if cfg!(target_arch = "x86_64") && cfg!(target_os = "linux") {
                "x86_64-unknown-linux-gnu"
            } else if cfg!(target_arch = "x86_64") && cfg!(target_os = "macos") {
                "x86_64-apple-darwin"
            } else if cfg!(target_arch = "aarch64") && cfg!(target_os = "macos") {
                "aarch64-apple-darwin"
            } else if cfg!(target_arch = "x86_64") && cfg!(target_os = "windows") {
                "x86_64-pc-windows-msvc"
            } else {
                "unknown"
            }
            .to_string()
        })
    });

    // Build settings
    let mut builder = SettingsBuilder::new()
        .project_out_directory(&out_dir)
        .package_settings(PackageSettings {
            product_name: override_name.clone().unwrap_or(name.clone()),
            version: override_version.clone().unwrap_or(version),
            description,
            homepage: None,
            authors: None,
            default_run: None,
        })
        .binaries(binaries.to_vec())
        .target(target);

    // Set package types if specified
    if let Some(platform_str) = platform {
        let package_type = parse_package_type(platform_str)?;
        builder = builder.package_types(vec![package_type]);
    }

    // Configure platform-specific settings
    use crate::bundler::{
        BundleSettings, DebianSettings, MacOsSettings, RpmSettings, WindowsSettings,
    };
    use std::path::PathBuf;

    // Read signing configuration from environment variables
    let macos_settings = MacOsSettings {
        signing_identity: std::env::var("MACOS_SIGNING_IDENTITY").ok(),
        entitlements: std::env::var("MACOS_ENTITLEMENTS_PATH")
            .ok()
            .map(PathBuf::from),
        skip_notarization: std::env::var("MACOS_SKIP_NOTARIZATION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(false),
        ..Default::default()
    };

    let windows_settings = WindowsSettings {
        cert_path: std::env::var("WINDOWS_CERT_PATH").ok().map(PathBuf::from),
        key_path: std::env::var("WINDOWS_KEY_PATH").ok().map(PathBuf::from),
        password: std::env::var("WINDOWS_CERT_PASSWORD").ok(),
        timestamp_url: std::env::var("WINDOWS_TIMESTAMP_URL").ok(),
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

    let bundle_settings = BundleSettings {
        identifier: Some(format!(
            "ai.kodegen.{}",
            override_name.clone().unwrap_or_else(|| name.clone())
        )),
        icon: Some(icon_paths),
        deb: DebianSettings {
            post_install_script: Some(PathBuf::from("packages/bundler-release/postinst.deb.sh")),
            ..Default::default()
        },
        rpm: RpmSettings {
            post_install_script: Some(PathBuf::from("packages/bundler-release/postinst.rpm.sh")),
            ..Default::default()
        },
        macos: macos_settings,
        windows: windows_settings,
        ..Default::default()
    };

    builder = builder.bundle_settings(bundle_settings);

    // Validate that all binaries exist before bundling
    let binary_dir = workspace.root.join(&out_dir);
    for binary in binaries {
        let binary_path = binary_dir.join(binary.name());
        let binary_path_exe = binary_dir.join(format!("{}.exe", binary.name()));

        if !binary_path.exists() && !binary_path_exe.exists() {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "validate_binaries".to_string(),
                reason: format!(
                    "Binary '{}' not found in {}\n\
                     Build all binaries first:\n  \
                     cargo build --release --bin kodegen_install\n  \
                     cargo build --release --bin kodegen\n  \
                     cargo build --release --bin kodegend",
                    binary.name(),
                    binary_dir.display()
                ),
            }));
        }
    }

    builder.build().map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "build_settings".to_string(),
            reason: e.to_string(),
        })
    })
}

/// Calculate SHA-256 checksum of a file
pub(super) fn calculate_artifact_checksum(path: &std::path::Path) -> Result<String> {
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

/// Parse package type string to enum
pub(super) fn parse_package_type(platform_str: &str) -> Result<crate::bundler::PackageType> {
    use crate::bundler::PackageType;

    match platform_str.to_lowercase().as_str() {
        "deb" | "debian" => Ok(PackageType::Deb),
        "rpm" => Ok(PackageType::Rpm),
        "appimage" => Ok(PackageType::AppImage),
        "app" | "macos" => Ok(PackageType::MacOsBundle),
        "dmg" => Ok(PackageType::Dmg),
        "nsis" => Ok(PackageType::Nsis),
        _ => Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Unknown package type: '{}'. Valid: deb, rpm, appimage, app, dmg, nsis",
                platform_str
            ),
        })),
    }
}

/// Print bundle creation summary
pub(super) fn print_bundle_summary(
    artifacts: &[crate::bundler::BundledArtifact],
    config: &RuntimeConfig,
) {
    if artifacts.is_empty() {
        config.warning_println("No artifacts were created");
        return;
    }

    config.success_println(&format!("Created {} package(s)", artifacts.len()));

    for artifact in artifacts {
        config.println(&format!("\n  {:?}:", artifact.package_type));
        for path in &artifact.paths {
            let size_mb = artifact.size as f64 / 1_048_576.0;
            config.println(&format!("    📦 {} ({:.2} MB)", path.display(), size_mb));
        }
        config.println(&format!("    🔐 SHA256: {}", artifact.checksum));
    }
}

/// Upload bundles to GitHub release
pub(super) async fn upload_bundles_to_github(
    workspace: &crate::workspace::WorkspaceInfo,
    artifacts: &[crate::bundler::BundledArtifact],
    github_repo: Option<&str>,
    git_manager: &GitManager,
    config: &RuntimeConfig,
) -> Result<()> {
    use super::super::helpers::{detect_github_repo, parse_github_repo_string};

    config.println("📤 Uploading artifacts to GitHub...");

    // Parse owner/repo
    let (owner, repo) = if let Some(repo_str) = github_repo {
        parse_github_repo_string(repo_str)?
    } else {
        // Detect from git remote origin
        detect_github_repo(git_manager).await?
    };

    // Get version from workspace
    let version = workspace
        .packages
        .values()
        .next()
        .ok_or_else(|| {
            ReleaseError::Cli(CliError::InvalidArguments {
                reason: "No workspace members found".to_string(),
            })
        })?
        .version
        .clone();

    // Initialize GitHub manager
    let github_config = crate::github::GitHubReleaseConfig {
        owner: owner.clone(),
        repo: repo.clone(),
        draft: false,
        prerelease_for_zero_versions: true,
        notes: None,
        token: None, // Will read from GH_TOKEN or GITHUB_TOKEN env var
    };

    let github_manager = crate::github::GitHubReleaseManager::new(github_config)?;

    // Create release if doesn't exist, or get existing
    let tag_name = format!("v{}", version);
    config.verbose_println(&format!("Looking for release {}", tag_name));

    // Get or create release
    let client = github_manager.client().inner().clone();
    let release =
        kodegen_tools_github::get_release_by_tag(client.clone(), &owner, &repo, &tag_name)
            .await
            .map_err(|e| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "get_release_by_tag".to_string(),
                    reason: e.to_string(),
                })
            })?;

    let release_id = if let Some(existing_release) = release {
        config.verbose_println(&format!(
            "Found existing release: {}",
            existing_release.html_url
        ));
        existing_release.id.0
    } else {
        // Create new release
        config.println(&format!("Creating release {}", tag_name));

        // Get current commit SHA
        let commit_sha = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "git_rev_parse".to_string(),
                    reason: e.to_string(),
                })
            })?;

        if !commit_sha.status.success() {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "git_rev_parse".to_string(),
                reason: "Failed to get current commit SHA".to_string(),
            }));
        }

        let sha = String::from_utf8_lossy(&commit_sha.stdout)
            .trim()
            .to_string();

        // Parse version string to semver::Version
        let semver_version = semver::Version::parse(&version.to_string()).map_err(|e| {
            ReleaseError::Cli(CliError::InvalidArguments {
                reason: format!("Invalid version '{}': {}", version, e),
            })
        })?;

        let result = github_manager
            .create_release(&semver_version, &sha, Some(format!("Release {}", version)))
            .await?;

        config.success_println(&format!("Created release: {}", result.html_url));
        result.release_id
    };

    // Collect all artifact paths
    let mut all_paths = Vec::new();
    for artifact in artifacts {
        all_paths.extend(artifact.paths.clone());
    }

    // Upload artifacts
    config.verbose_println(&format!("Uploading {} files", all_paths.len()));
    let uploaded_urls = github_manager
        .upload_artifacts(release_id, &all_paths, config)
        .await?;

    config.success_println(&format!(
        "Uploaded {} artifact(s) to GitHub",
        uploaded_urls.len()
    ));
    for url in &uploaded_urls {
        config.println(&format!("  📦 {}", url));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::WorkspaceInfo;

    #[test]
    fn test_discover_binaries_from_workspace() {
        // Analyze actual workspace (relative to bundler-release package)
        let workspace = WorkspaceInfo::analyze("../..").expect("Failed to analyze workspace");

        println!("Workspace packages: {}", workspace.packages.len());
        for (name, pkg) in &workspace.packages {
            println!("  Package: {} at {:?}", name, pkg.cargo_toml_path);
        }

        // Run discovery
        match discover_binaries_from_workspace(&workspace) {
            Ok(binaries) => {
                println!("\n✓ Found {} binaries:", binaries.len());
                for binary in &binaries {
                    println!("  - {} (main: {})", binary.name(), binary.main());
                }

                // Verify kodegen_install found and marked as main
                let install_binary = binaries
                    .iter()
                    .find(|b| b.name() == "kodegen_install")
                    .expect("kodegen_install binary not found");
                assert!(
                    install_binary.main(),
                    "kodegen_install should be marked as main"
                );

                // Verify other key binaries found
                assert!(
                    binaries.iter().any(|b| b.name() == "kodegen"),
                    "kodegen binary not found"
                );
                assert!(
                    binaries.iter().any(|b| b.name() == "kodegend"),
                    "kodegend binary not found"
                );

                // Verify only one binary marked as main
                let main_count = binaries.iter().filter(|b| b.main()).count();
                assert_eq!(
                    main_count, 1,
                    "Exactly one binary should be marked as main"
                );
            }
            Err(e) => {
                println!("\n✗ Binary discovery failed: {:?}", e);
                println!(
                    "\nThis might be expected if no packages have [[bin]] sections in this view"
                );
                panic!("Binary discovery failed: {:?}", e);
            }
        }
    }
}

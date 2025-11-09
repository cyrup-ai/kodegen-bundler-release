//! Platform detection and bundling logic for release artifacts.

use crate::error::{CliError, ReleaseError, Result};

use super::context::ReleasePhaseContext;

/// Get all platforms to build for release
pub fn get_platforms_to_build() -> Vec<&'static str> {
    // Return all supported platforms
    // The bundler will automatically use Docker for cross-platform builds
    vec!["deb", "rpm", "appimage", "dmg", "nsis"]
}

/// Get platforms that can be built natively on current OS
pub fn get_native_platforms<'a>(all_platforms: &'a [&'a str]) -> Vec<&'a str> {
    all_platforms
        .iter()
        .copied()
        .filter(|p| is_native_platform(p))
        .collect()
}

/// Get platforms that require Docker on current OS
pub fn get_docker_platforms<'a>(all_platforms: &'a [&'a str]) -> Vec<&'a str> {
    all_platforms
        .iter()
        .copied()
        .filter(|p| !is_native_platform(p))
        .collect()
}

/// Check if platform can be built natively on current OS
///
/// Uses runtime platform detection via `std::env::consts::OS` instead of
/// compile-time cfg attributes, enabling universal binaries.
pub fn is_native_platform(platform: &str) -> bool {
    match (std::env::consts::OS, platform) {
        // macOS native packages
        ("macos", "dmg") => true,

        // Linux native packages  
        ("linux", "deb" | "rpm" | "appimage") => true,

        // Windows native packages
        ("windows", "nsis") => true,

        // Everything else requires Docker
        _ => false,
    }
}

/// Determine the architecture string for the current build target
/// 
/// This reads the actual target architecture from the build context,
/// not from hardcoded assumptions.
pub fn detect_target_architecture() -> Result<&'static str> {
    // On macOS, check if building for ARM or Intel
    #[cfg(target_os = "macos")]
    {
        #[cfg(target_arch = "aarch64")]
        return Ok("arm64");
        
        #[cfg(target_arch = "x86_64")]
        return Ok("x86_64");
    }
    
    // On Linux, check target architecture
    #[cfg(target_os = "linux")]
    {
        #[cfg(target_arch = "aarch64")]
        return Ok("arm64");
        
        #[cfg(target_arch = "x86_64")]
        return Ok("amd64");
        
        #[cfg(target_arch = "x86")]
        return Ok("i386");
    }
    
    // On Windows
    #[cfg(target_os = "windows")]
    {
        #[cfg(target_arch = "x86_64")]
        return Ok("x64");
        
        #[cfg(target_arch = "x86")]
        return Ok("x86");
        
        #[cfg(target_arch = "aarch64")]
        return Ok("arm64");
    }
    
    #[allow(unreachable_code)]
    Err(ReleaseError::Cli(CliError::InvalidArguments {
        reason: format!("Unsupported target architecture: {}", std::env::consts::ARCH),
    }))
}

/// Construct the output filename for a platform artifact
/// 
/// Includes the actual target architecture in the filename.
pub fn construct_output_filename(
    ctx: &ReleasePhaseContext<'_>,
    platform: &str,
    arch: &str,
) -> Result<String> {
    let product_name = ctx.binary_name;
    let version = ctx.new_version.to_string();
    
    let filename = match platform {
        "deb" => format!("{}_{}_{}.deb", product_name, version, arch),
        "rpm" => format!("{}-{}-1.{}.rpm", product_name, version, arch),
        "dmg" => format!("{}-{}-{}.dmg", product_name, version, arch),
        "nsis" | "exe" => format!("{}_{}_{}_setup.exe", product_name, version, arch),
        "appimage" => format!("{}-{}-{}.AppImage", product_name, version, arch),
        _ => {
            return Err(ReleaseError::Cli(CliError::InvalidArguments {
                reason: format!("Unknown platform: {}", platform),
            }));
        }
    };
    
    Ok(filename)
}

/// Ensure bundler binary is installed from GitHub
///
/// Uses cargo install to fetch from GitHub. Cargo automatically:
/// - Checks GitHub for new commits (~0.7s)
/// - Skips if already up-to-date
/// - Rebuilds only if new commits exist
pub async fn ensure_bundler_installed(ctx: &ReleasePhaseContext<'_>) -> Result<std::path::PathBuf> {
    ctx.config.verbose_println("   Checking bundler installation from GitHub...");

    let install_status = std::process::Command::new("cargo")
        .arg("install")
        .arg("--git")
        .arg("https://github.com/cyrup-ai/kodegen-bundler-bundle")
        .arg("kodegen_bundler_bundle")
        .status()
        .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
            command: "cargo install bundler".to_string(),
            reason: e.to_string(),
        }))?;

    if !install_status.success() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "cargo install bundler".to_string(),
            reason: format!("Install failed with exit code: {:?}", install_status.code()),
        }));
    }

    ctx.config.verbose_println("   ✓ Bundler ready");

    // Return command name - cargo install puts it in PATH
    Ok(std::path::PathBuf::from("kodegen_bundler_bundle"))
}

/// Bundle a single platform using native bundler binary
pub async fn bundle_native_platform(
    ctx: &ReleasePhaseContext<'_>,
    bundler_binary: &std::path::PathBuf,
    platform: &str,
) -> Result<Vec<std::path::PathBuf>> {
    // Detect target architecture
    // For DMG on macOS, check if universal binaries are available
    let arch = {
        #[cfg(target_os = "macos")]
        if platform == "dmg" {
            let universal_dir = ctx.release_clone_path.join("target/universal/release");
            if universal_dir.exists() {
                "universal"
            } else {
                detect_target_architecture()?
            }
        } else {
            detect_target_architecture()?
        }

        #[cfg(not(target_os = "macos"))]
        detect_target_architecture()?
    };

    // Construct output path with explicit architecture
    // Note: We do NOT create the artifacts directory - bundler handles this
    let filename = construct_output_filename(ctx, platform, arch)?;
    let output_path = ctx.release_clone_path.join("artifacts").join(&filename);
    
    ctx.config.verbose_println(&format!(
        "   Target architecture: {}\n   Output path: {}",
        arch,
        output_path.display()
    ));
    
    // Call bundler with explicit output path
    // Bundler will create parent directories and move artifact there
    // NOTE: Version is read from Cargo.toml in release_clone_path, not passed as argument
    let mut cmd = std::process::Command::new(bundler_binary);
    cmd.arg("--repo-path")
        .arg(ctx.release_clone_path)
        .arg("--platform")
        .arg(platform)
        .arg("--binary-name")
        .arg(ctx.binary_name)
        .arg("--output-binary")
        .arg(&output_path)
        .arg("--no-build");

    // For universal binaries, pass --target universal to bundler
    if arch == "universal" {
        cmd.arg("--target").arg("universal");
        ctx.config.verbose_println("   Using universal binaries for DMG");
    }

    let output = cmd.output()
        .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: e.to_string(),
        }))?;
    
    // Capture stdout and stderr for diagnostics
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    if ctx.config.is_verbose() && !stdout.is_empty() {
        ctx.config.verbose_println(&format!("   Bundler stdout:\n{}", stdout));
    }
    
    if !stderr.is_empty() {
        ctx.config.verbose_println(&format!("   Bundler stderr:\n{}", stderr));
    }
    
    if !output.status.success() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: format!("Bundling failed (exit code {:?}):\n{}", output.status.code(), stderr),
        }));
    }
    
    // Contract enforcement: exit code 0 means artifact exists at output_path
    if !output_path.exists() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: format!(
                "Bundler exit code 0 but artifact not found at {}.\n\
                 This is a contract violation in the bundler.",
                output_path.display()
            ),
        }));
    }
    
    ctx.config.indent(&format!("✓ {}", filename));
    
    Ok(vec![output_path])
}

/// Bundle a single platform using Docker (via bundler binary)
///
/// The bundler binary itself handles Docker internally for cross-platform builds.
/// We just call the bundler binary the same way as native platforms.
pub async fn bundle_docker_platform(
    ctx: &ReleasePhaseContext<'_>,
    bundler_binary: &std::path::PathBuf,
    platform: &str,
) -> Result<Vec<std::path::PathBuf>> {
    // For Docker platforms, architecture depends on the Docker target
    // For Linux platforms in Docker, we're typically building for x86_64/amd64
    let arch = match platform {
        "deb" | "rpm" => "amd64",  // Default Docker Linux target
        "appimage" => "x86_64",
        "nsis" => "x64",  // Windows 64-bit
        _ => {
            return Err(ReleaseError::Cli(CliError::InvalidArguments {
                reason: format!("Docker bundling not supported for platform: {}", platform),
            }));
        }
    };
    
    // Construct output path with explicit architecture
    // Note: We do NOT create the artifacts directory - bundler handles this
    let filename = construct_output_filename(ctx, platform, arch)?;
    let output_path = ctx.release_clone_path.join("artifacts").join(&filename);
    
    ctx.config.verbose_println(&format!(
        "   Docker target architecture: {}\n   Output path: {}",
        arch,
        output_path.display()
    ));
    
    // Call bundler with explicit output path (bundler handles Docker internally)
    // Bundler will create parent directories and move artifact there
    // NOTE: Version is read from Cargo.toml in release_clone_path, not passed as argument
    // NOTE: We do NOT pass --no-build for Docker platforms because the bundler needs to
    //       build Linux binaries inside the container (we only built macOS binaries on host)
    let output = std::process::Command::new(bundler_binary)
        .arg("--repo-path")
        .arg(ctx.release_clone_path)
        .arg("--platform")
        .arg(platform)
        .arg("--binary-name")
        .arg(ctx.binary_name)
        .arg("--output-binary")
        .arg(&output_path)  // ← CALLER SPECIFIES PATH
        .output()
        .map_err(|e| ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: e.to_string(),
        }))?;
    
    // Capture stdout and stderr for diagnostics
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    if ctx.config.is_verbose() && !stdout.is_empty() {
        ctx.config.verbose_println(&format!("   Bundler stdout:\n{}", stdout));
    }
    
    if !stderr.is_empty() {
        ctx.config.verbose_println(&format!("   Bundler stderr:\n{}", stderr));
    }
    
    if !output.status.success() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: format!("Docker bundling failed (exit code {:?}):\n{}", output.status.code(), stderr),
        }));
    }
    
    // Contract enforcement: exit code 0 means artifact exists at output_path
    if !output_path.exists() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: format!(
                "Bundler exit code 0 but artifact not found at {}.\n\
                 This is a contract violation in the bundler.",
                output_path.display()
            ),
        }));
    }
    
    ctx.config.indent(&format!("✓ {}", filename));
    
    Ok(vec![output_path])
}

//! Platform detection and bundling logic for release artifacts.

use crate::error::{CliError, ReleaseError, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::context::ReleasePhaseContext;

/// Get all platforms to build for release
pub fn get_platforms_to_build() -> Vec<&'static str> {
    // Return all supported platforms
    // The bundler will automatically use Docker for cross-platform builds
    vec!["deb", "rpm", "appimage", "dmg", "exe"]
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
        ("windows", "exe") => true,

        // Everything else requires Docker
        _ => false,
    }
}

/// Determine the architecture string for the current build target
/// 
/// This reads the actual target architecture from the build context.
pub fn detect_target_architecture() -> Result<&'static str> {
    #[cfg(target_os = "macos")]
    {
        #[cfg(target_arch = "aarch64")]
        return Ok("arm64");
        
        #[cfg(target_arch = "x86_64")]
        return Ok("x86_64");
    }
    
    #[cfg(target_os = "linux")]
    {
        #[cfg(target_arch = "aarch64")]
        return Ok("arm64");
        
        #[cfg(target_arch = "x86_64")]
        return Ok("amd64");
        
        #[cfg(target_arch = "x86")]
        return Ok("i386");
    }
    
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
    binary_name: &str,
    version: &str,
    platform: &str,
    arch: &str,
) -> Result<String> {
    let filename = match platform {
        "deb" => format!("{}_{}_{}.deb", binary_name, version, arch),
        "rpm" => format!("{}-{}-1.{}.rpm", binary_name, version, arch),
        "dmg" => format!("{}-{}-{}.dmg", binary_name, version, arch),
        "exe" => format!("{}_{}_{}_setup.exe", binary_name, version, arch),
        "appimage" => format!("{}-{}-{}.AppImage", binary_name, version, arch),
        _ => {
            return Err(ReleaseError::Cli(CliError::InvalidArguments {
                reason: format!("Unknown platform: {}", platform),
            }));
        }
    };
    
    Ok(filename)
}

/// Get installed binary version by running `binary --version`
async fn get_installed_version(binary_name: &str) -> Option<String> {
    let output = tokio::process::Command::new(binary_name)
        .arg("--version")
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse version using regex (matches semantic versions like 0.10.0)
    let re = regex::Regex::new(r"\b(\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?)\b").ok()?;
    re.captures(&stdout)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

/// Get latest version from crates.io API
async fn get_crates_io_version(crate_name: &str) -> Option<String> {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct CratesIoResponse {
        #[serde(rename = "crate")]
        crate_data: CrateData,
    }

    #[derive(Deserialize)]
    struct CrateData {
        max_version: String,
    }

    let url = format!("https://crates.io/api/v1/crates/{}", crate_name);
    let client = reqwest::Client::builder()
        .user_agent("kodegen_bundler_release")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let response = client.get(&url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    let data: CratesIoResponse = response.json().await.ok()?;
    Some(data.crate_data.max_version)
}

/// Ensure bundler binary is installed from crates.io
///
/// Smart installation logic (same as kodegend):
/// 1. Check if kodegen_bundler_bundle is installed locally
/// 2. Get its version and compare with latest on crates.io
/// 3. Only install/update if local version is missing or older
///
/// This avoids unnecessary reinstalls during development.
pub async fn ensure_bundler_installed(ctx: &ReleasePhaseContext<'_>) -> Result<std::path::PathBuf> {
    let binary_name = "kodegen_bundler_bundle";

    // Get installed version
    let installed_version = get_installed_version(binary_name).await;

    // Get latest crates.io version
    let latest_version = get_crates_io_version(binary_name).await;

    let needs_install = match (&installed_version, &latest_version) {
        (None, _) => {
            ctx.config.verbose_println(&format!("   {} not found, installing...", binary_name)).expect("Failed to write to stdout");
            true
        }
        (Some(_), None) => {
            ctx.config.verbose_println("   Could not check crates.io, using installed version").expect("Failed to write to stdout");
            false // Can't check crates.io, assume installed is OK
        }
        (Some(installed), Some(latest)) => {
            use semver::Version;
            match (Version::parse(installed), Version::parse(latest)) {
                (Ok(inst_ver), Ok(lat_ver)) => {
                    if inst_ver >= lat_ver {
                        ctx.config.verbose_println(&format!("   ✓ Bundler already installed: v{}", installed)).expect("Failed to write to stdout");
                        false
                    } else {
                        ctx.config.verbose_println(&format!("   Updating bundler: v{} → v{}", installed, latest)).expect("Failed to write to stdout");
                        true
                    }
                }
                _ => false, // Parse error, assume installed is OK
            }
        }
    };

    if !needs_install {
        return Ok(std::path::PathBuf::from(binary_name));
    }

    // Install from crates.io
    ctx.config.verbose_println(&format!("   Installing {} from crates.io...", binary_name)).expect("Failed to write to stdout");

    let install_status = std::process::Command::new("cargo")
        .arg("install")
        .arg(binary_name)
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

    ctx.config.verbose_println("   ✓ Bundler installed successfully").expect("Failed to write to stdout");

    Ok(std::path::PathBuf::from(binary_name))
}

/// Bundle a platform by invoking kodegen_bundler_bundle binary
///
/// Uses ONLY the 3 required arguments with proper stdout/stderr streaming.
pub async fn bundle_platform(
    ctx: &ReleasePhaseContext<'_>,
    bundler_binary: &std::path::PathBuf,
    platform: &str,
) -> Result<Vec<std::path::PathBuf>> {
    // Determine architecture for filename construction
    let arch = match platform {
        // Docker platforms have fixed architectures
        "deb" | "rpm" => "amd64",
        "appimage" => "x86_64",
        "exe" => "x64",

        // Native platforms use detected architecture
        "dmg" => detect_target_architecture()?,
        
        _ => {
            return Err(ReleaseError::Cli(CliError::InvalidArguments {
                reason: format!("Unsupported platform: {}", platform),
            }));
        }
    };

    // Construct output path with architecture
    let filename = construct_output_filename(
        ctx.binary_name,
        &ctx.new_version.to_string(),
        platform,
        arch,
    )?;
    let output_path = ctx.release_clone_path.join("artifacts").join(&filename);
    
    ctx.config.verbose_println(&format!(
        "   Target architecture: {}\n   Output path: {}",
        arch,
        output_path.display()
    )).expect("Failed to write to stdout");

    // Determine source argument
    // Bundler needs GitHub URL to clone - construct from metadata
    let github_url = format!(
        "https://github.com/{}/{}",
        ctx.github_owner,
        ctx.github_repo_name
    );
    
    // Invoke bundler with ONLY 3 arguments
    let mut child = Command::new(bundler_binary)
        .arg("--source")
        .arg(&github_url)
        .arg("--platform")
        .arg(platform)
        .arg("--output-binary")
        .arg(&output_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: format!("bundle_{}", platform),
                reason: e.to_string(),
            })
        })?;

    // Stream stdout and stderr concurrently through OutputManager
    let runtime_config = ctx.config.clone();
    let runtime_config2 = ctx.config.clone();
    
    tokio::join!(
        async {
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    runtime_config.indent(&line).expect("Failed to write to stdout");
                }
            }
        },
        async {
            if let Some(stderr) = child.stderr.take() {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    runtime_config2.indent(&line).expect("Failed to write to stdout");
                }
            }
        }
    );

    // Wait for process to complete
    let status = child.wait().await.map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: e.to_string(),
        })
    })?;
    
    // Contract enforcement: exit code 0 = file guaranteed to exist
    if status.success() {
        if !output_path.exists() {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: format!("bundle_{}", platform),
                reason: format!(
                    "CONTRACT VIOLATION: Bundler returned exit 0 but artifact not found at {}\n\
                     This indicates a bug in kodegen_bundler_bundle.",
                    output_path.display()
                ),
            }));
        }

        ctx.config.indent(&format!("✓ {}", filename)).expect("Failed to write to stdout");
        Ok(vec![output_path])
    } else {
        Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: format!("bundle_{}", platform),
            reason: format!(
                "Bundling failed with exit code {:?}",
                status.code()
            ),
        }))
    }
}

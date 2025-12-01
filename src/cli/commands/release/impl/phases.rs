//! Release phase execution.
//!
//! Handles GitHub release creation, building, bundling, and artifact upload.
//! Git operations and cargo publish are handled by `just publish` before this runs.

use crate::error::{CliError, ReleaseError, Result};
use crate::state::ReleaseState;
use crate::EnvConfig;

use super::context::ReleasePhaseContext;
use super::platform::{
    bundle_platform, ensure_bundler_installed, get_docker_platforms, get_native_platforms,
    get_platforms_to_build,
};
use super::retry::retry_with_backoff;

/// Get environment variables needed for native cross-compilation to the specified target.
/// Extracts OpenSSL, pkg-config, and other build-related vars from EnvConfig.
///
/// This is only needed for native macOS cross-compilation (arm64 ↔ x86_64).
/// Docker-based builds (Linux, Windows) manage their own environment.
fn get_cross_compile_env(target: &str, env_config: &EnvConfig) -> Vec<(String, String)> {
    let mut env = Vec::new();

    // Target-specific OpenSSL variables (e.g., X86_64_APPLE_DARWIN_OPENSSL_DIR)
    let target_upper = target.to_uppercase().replace('-', "_");
    let openssl_keys = [
        format!("{}_OPENSSL_DIR", target_upper),
        format!("{}_OPENSSL_LIB_DIR", target_upper),
        format!("{}_OPENSSL_INCLUDE_DIR", target_upper),
        "OPENSSL_DIR".to_string(),
        "OPENSSL_LIB_DIR".to_string(),
        "OPENSSL_INCLUDE_DIR".to_string(),
    ];

    for key in &openssl_keys {
        if let Some(value) = env_config.get(key) {
            env.push((key.clone(), value));
        }
    }

    // pkg-config variables for cross-compilation
    let pkg_config_keys = [
        format!("PKG_CONFIG_PATH_{}", target_upper),
        format!("PKG_CONFIG_SYSROOT_DIR_{}", target_upper),
        "PKG_CONFIG_ALLOW_CROSS".to_string(),
    ];

    for key in &pkg_config_keys {
        if let Some(value) = env_config.get(key) {
            env.push((key.clone(), value));
        }
    }

    // Other common cross-compile vars
    for key in ["CC", "CXX", "AR", "RANLIB"] {
        if let Some(value) = env_config.get(key) {
            env.push((key.to_string(), value));
        }
    }

    env
}

/// Execute release phases with retry logic
///
/// Phases:
/// 1. Create GitHub draft release (using existing tag)
/// 2. Build release binaries
/// 3. Create platform bundles
/// 4. Upload artifacts incrementally
/// 5. Publish GitHub release
pub async fn execute_phases_with_retry(
    ctx: &ReleasePhaseContext<'_>,
    release_state: &mut ReleaseState,
    env_config: &crate::EnvConfig,
) -> Result<()> {
    use crate::cli::retry_config::{CargoTimeoutConfig, RetryConfig};
    let retry_config = RetryConfig::default();
    let timeout_config = CargoTimeoutConfig::default();

    // ===== PHASE 1: CREATE GITHUB DRAFT RELEASE =====
    let release_id = if release_state.has_completed(crate::state::ReleasePhase::GitHubRelease) {
        ctx.config
            .println("✓ Skipping GitHub release creation (already completed)")
            .expect("Failed to write to stdout");
        if let Some(ref github_state) = release_state.github_state {
            ctx.config
                .indent(&format!(
                    "   Release: {}",
                    github_state.html_url.as_ref().unwrap_or(&"N/A".to_string())
                ))
                .expect("Failed to write to stdout");
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
        ctx.config
            .println("🚀 Creating GitHub draft release...")
            .expect("Failed to write to stdout");

        // Use the existing tag (created by `just publish`)
        let tag_name = format!("v{}", ctx.new_version);

        let release_result = retry_with_backoff(
            || ctx.github_manager.create_release_from_tag(ctx.new_version, &tag_name, None),
            retry_config.github_api,
            "GitHub release creation",
            ctx.config,
            None,
        )
        .await?;

        ctx.config
            .success_println(&format!(
                "✓ Created draft release: {}",
                release_result.html_url
            ))
            .expect("Failed to write to stdout");

        // Track release in state
        release_state.set_github_state(
            ctx.github_owner.to_string(),
            ctx.github_repo_name.to_string(),
            Some(&release_result),
        );
        let release_id = release_result.release_id;

        // Save state
        release_state.set_phase(crate::state::ReleasePhase::GitHubRelease);
        release_state.add_checkpoint(
            "github_release_created".to_string(),
            crate::state::ReleasePhase::GitHubRelease,
            Some(serde_json::json!({
                "release_id": release_id,
                "html_url": &release_result.html_url,
            })),
        );
        crate::state::save_release_state(ctx.release_clone_path, release_state).await?;
        ctx.config
            .verbose_println("ℹ️  Saved progress checkpoint (GitHub release)")
            .expect("Failed to write to stdout");

        release_id
    };

    // ===== PHASE 2: BUILD RELEASE BINARIES =====
    ctx.config
        .println("🔨 Building release binaries...")
        .expect("Failed to write to stdout");

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
                .output(),
        )
        .await
        .map_err(|_| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "cargo build --release".to_string(),
                reason: format!(
                    "Build timed out after {} seconds",
                    timeout_config.build_timeout_secs
                ),
            })
        })?
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "cargo build --release".to_string(),
                reason: e.to_string(),
            })
        })?;

        if !build_output.status.success() {
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: "cargo build --release".to_string(),
                reason: String::from_utf8_lossy(&build_output.stderr).to_string(),
            }));
        }
    } else {
        // Multi-target build (macOS) - propagate cross-compile env vars
        for target in &build_targets {
            ctx.config
                .verbose_println(&format!("   Building for {}...", target))
                .expect("Failed to write to stdout");

            let cross_env = get_cross_compile_env(target, env_config);
            let build_output = timeout(
                build_timeout,
                Command::new("cargo")
                    .arg("build")
                    .arg("--release")
                    .arg("--target")
                    .arg(target)
                    .current_dir(ctx.release_clone_path)
                    .envs(cross_env)
                    .output(),
            )
            .await
            .map_err(|_| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: format!("cargo build --release --target {}", target),
                    reason: format!(
                        "Build timed out after {} seconds",
                        timeout_config.build_timeout_secs
                    ),
                })
            })?
            .map_err(|e| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: format!("cargo build --release --target {}", target),
                    reason: e.to_string(),
                })
            })?;

            if !build_output.status.success() {
                return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                    command: format!("cargo build --release --target {}", target),
                    reason: String::from_utf8_lossy(&build_output.stderr).to_string(),
                }));
            }
        }
    }

    ctx.config
        .success_println("✓ Built release binaries")
        .expect("Failed to write to stdout");

    release_state.set_phase(crate::state::ReleasePhase::Building);
    crate::state::save_release_state(ctx.release_clone_path, release_state).await?;

    // ===== PHASE 3: CREATE PLATFORM BUNDLES =====
    ctx.config
        .println("📦 Creating platform bundles...")
        .expect("Failed to write to stdout");

    let all_platforms = get_platforms_to_build();
    ctx.config
        .verbose_println(&format!(
            "   Building {} platform(s)",
            all_platforms.len()
        ))
        .expect("Failed to write to stdout");

    let native_platforms = get_native_platforms(&all_platforms);
    let docker_platforms = get_docker_platforms(&all_platforms);

    ctx.config
        .verbose_println(&format!(
            "   Native: {} platform(s), Docker: {} platform(s)",
            native_platforms.len(),
            docker_platforms.len()
        ))
        .expect("Failed to write to stdout");

    let mut total_artifacts_created = 0;
    let mut total_artifacts_uploaded = 0;

    if !all_platforms.is_empty() {
        let bundler_binary = ensure_bundler_installed(ctx).await?;

        for platform in &all_platforms {
            let is_native = native_platforms.contains(platform);
            let platform_type = if is_native { "native" } else { "Docker" };

            ctx.config
                .verbose_println(&format!("\n   Building {} ({})...", platform, platform_type))
                .expect("Failed to write to stdout");

            let artifacts = bundle_platform(ctx, &bundler_binary, platform).await?;

            total_artifacts_created += artifacts.len();

            // Upload immediately after bundling
            let uploaded = upload_artifacts_incrementally(
                ctx,
                release_state,
                release_id,
                &artifacts,
                platform,
            )
            .await?;

            total_artifacts_uploaded += uploaded;
        }
    }

    if total_artifacts_created == 0 {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "bundle".to_string(),
            reason: "No artifacts were created".to_string(),
        }));
    }

    ctx.config
        .success_println(&format!(
            "✓ Created {} artifact(s) across {} platform(s)",
            total_artifacts_created,
            all_platforms.len()
        ))
        .expect("Failed to write to stdout");
    ctx.config
        .success_println(&format!(
            "✓ Uploaded {} artifact(s) to GitHub release",
            total_artifacts_uploaded
        ))
        .expect("Failed to write to stdout");

    release_state.set_phase(crate::state::ReleasePhase::Uploading);
    crate::state::save_release_state(ctx.release_clone_path, release_state).await?;

    // ===== PHASE 4: PUBLISH GITHUB RELEASE =====
    if release_state.has_completed(crate::state::ReleasePhase::GitHubPublish) {
        ctx.config
            .println("✓ Skipping release publishing (already published)")
            .expect("Failed to write to stdout");
    } else {
        ctx.config
            .println("🔍 Verifying release is ready to publish...")
            .expect("Failed to write to stdout");

        match ctx.github_manager.verify_release_is_draft(release_id).await {
            Ok(true) => {
                ctx.config
                    .success_println("✓ Release verified as draft")
                    .expect("Failed to write to stdout");
            }
            Ok(false) => {
                return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "publish_release".to_string(),
                    reason: format!(
                        "Release {} is not a draft (already published)",
                        release_id
                    ),
                }));
            }
            Err(e) => {
                return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "verify_release_draft_status".to_string(),
                    reason: format!("Failed to verify release {} draft status: {}", release_id, e),
                }));
            }
        }

        ctx.config
            .println("✅ Publishing GitHub release...")
            .expect("Failed to write to stdout");

        retry_with_backoff(
            || ctx.github_manager.publish_draft_release(release_id),
            retry_config.release_publishing,
            "Publish GitHub release",
            ctx.config,
            None,
        )
        .await?;

        ctx.config
            .success_println(&format!("✓ Published release v{}", ctx.new_version))
            .expect("Failed to write to stdout");

        release_state.set_phase(crate::state::ReleasePhase::GitHubPublish);
        release_state.add_checkpoint(
            "release_published".to_string(),
            crate::state::ReleasePhase::GitHubPublish,
            None,
        );
        crate::state::save_release_state(ctx.release_clone_path, release_state).await?;
    }

    release_state.set_phase(crate::state::ReleasePhase::Completed);
    crate::state::save_release_state(ctx.release_clone_path, release_state).await?;

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

        // Check if already uploaded
        let already_uploaded = release_state
            .github_state
            .as_ref()
            .map(|gh| gh.uploaded_artifacts.contains(&filename.to_string()))
            .unwrap_or(false);

        if already_uploaded {
            ctx.config
                .indent(&format!("⏭ {} (already uploaded)", filename))
                .expect("Failed to write to stdout");
            continue;
        }

        ctx.config
            .indent(&format!("☁️  Uploading {}...", filename))
            .expect("Failed to write to stdout");

        let uploaded_urls = ctx
            .github_manager
            .upload_artifacts(
                release_id,
                std::slice::from_ref(artifact_path),
                ctx.new_version,
                ctx.config,
            )
            .await
            .map_err(|e| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: format!("upload_{}", platform),
                    reason: e.to_string(),
                })
            })?;

        if !uploaded_urls.is_empty() {
            if let Some(github_state) = &mut release_state.github_state {
                github_state.uploaded_artifacts.push(filename.to_string());
            }

            crate::state::save_release_state(ctx.release_clone_path, release_state).await?;

            ctx.config
                .indent(&format!("✓ Uploaded {}", filename))
                .expect("Failed to write to stdout");
            uploaded_count += 1;
        }
    }

    Ok(uploaded_count)
}

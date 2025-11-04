//! Release command execution module.
//!
//! Handles the complete release workflow by coordinating all modules
//! in an isolated temporary clone to prevent modifications to the user's working directory.

mod r#impl;

use crate::cli::{Args, RuntimeConfig};
use crate::error::{CliError, ReleaseError, Result};

/// Options for configuring the release process
#[derive(Clone)]
pub(super) struct ReleaseOptions {
    pub bump_type: crate::cli::args::BumpType,
    pub dry_run: bool,
    pub no_push: bool,
    pub registry: Option<String>,
    pub github_repo: Option<String>,
}



/// Execute release command
pub(super) async fn execute_release(args: &Args, config: &RuntimeConfig) -> Result<i32> {
    // 1. Parse and resolve repository source
    config.println("📦 Resolving repository source...");
    let source_parsed = crate::source::RepositorySource::parse(&args.source)?;
    let resolved = source_parsed.resolve().await?;
    config.verbose_println(&format!("✓ Repository: {}", resolved.path.display()));

    // 2. Extract metadata from single Cargo.toml (NO workspace analysis)
    let cargo_toml = resolved.path.join("Cargo.toml");
    let metadata = crate::metadata::extract_metadata(&cargo_toml)?;
    let binary_name = crate::metadata::discover_binary(&cargo_toml)?;
    
    config.verbose_println(&format!("✓ Package: {}", metadata.name));
    config.verbose_println(&format!("✓ Binary: {}", binary_name));

    // 3. Simple validation (git status check - NO workspace validation)
    if !args.skip_validation {
        config.println("🔍 Validating repository...");
        // Simple git check - repository should be clean
        let git_status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&resolved.path)
            .output()
            .map_err(|e| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "git_status".to_string(),
                    reason: e.to_string(),
                })
            })?;
        
        if !git_status.stdout.is_empty() {
            config.warning_println("⚠️  Working directory has uncommitted changes");
            config.warning_println("   This may cause issues with the release process");
        }
    }

    // 4. Create temp clone for isolated execution
    config.println("📁 Creating temporary clone...");
    let temp_dir = if resolved.is_temp {
        resolved.path.clone()
    } else {
        super::temp_clone::clone_main_to_temp_for_release(&resolved.path).await?
    };
    let temp_dir_pathbuf = temp_dir.to_path_buf();

    // 5. Build release options (simplified)
    let options = ReleaseOptions {
        bump_type: args.bump_type.clone(),
        dry_run: args.dry_run,
        no_push: args.no_push,
        registry: args.registry.clone(),
        github_repo: args.github_repo.clone(),
    };

    // 6. Execute release in temp (NO workspace parameter)
    let result = r#impl::perform_release_single_repo(
        &temp_dir_pathbuf,
        metadata,
        binary_name,
        config,
        &options
    ).await;

    // 7. Cleanup temp directory
    if !args.dry_run && !resolved.is_temp {
        match std::fs::remove_dir_all(&temp_dir_pathbuf) {
            Ok(()) => {
                config.verbose_println("✅ Temp clone cleaned up");
            }
            Err(e) => {
                config.warning_println(&format!("Failed to cleanup temp directory: {}", e));
                config.warning_println(&format!(
                    "You may need to manually remove: {}",
                    temp_dir_pathbuf.display()
                ));
            }
        }
    }

    result
}

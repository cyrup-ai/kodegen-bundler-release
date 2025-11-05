//! Cyrup Release - Production-quality release management for Rust workspaces.
//!
//! This binary provides atomic release operations with proper error handling,
//! automatic internal dependency version synchronization, and rollback capabilities.

use kodegen_bundler_release::cli;
use kodegen_bundler_release::cli::OutputManager;
use std::process;

fn main() {
    // CRITICAL: Parse and set environment variables FIRST (guaranteed single-threaded)
    // This MUST be the very first operation in main() to avoid UB from set_var
    parse_and_set_zshrc_env_vars();

    // NOW safe to initialize logging (after env vars are set)
    env_logger::init();

    // Create tokio runtime
    let runtime = tokio::runtime::Runtime::new()
        .expect("Failed to create Tokio runtime");

    // Run the async main logic
    let exit_code = runtime.block_on(async_main());
    process::exit(exit_code);
}

/// Parse ~/.zshrc and set environment variables
/// 
/// SAFETY: This function uses `unsafe { std::env::set_var() }`, which is only safe
/// when called from a single-threaded context. This function MUST be called before
/// creating the Tokio runtime to avoid undefined behavior.
fn parse_and_set_zshrc_env_vars() {
    // Allow skipping .zshrc sourcing if problematic
    // Useful for: CI environments, debugging, or when .zshrc has issues
    if std::env::var("KODEGEN_SKIP_ZSHRC").is_ok() {
        return;
    }
    
    // Source ~/.zshrc to load environment variables (APPLE_CERTIFICATE, etc.)
    // This is critical for code signing to work properly
    let Some(home) = dirs::home_dir() else {
        return;
    };

    let zshrc = home.join(".zshrc");
    if !zshrc.exists() {
        return;
    }

    // Use null-byte separators for unambiguous parsing
    // This handles all edge cases: newlines in values, '=' in values, empty values, etc.
    let script = format!(
        r#"source {} && env | while IFS='=' read -r key value; do printf '%s\0%s\0' "$key" "$value"; done"#,
        zshrc.display()
    );

    let Ok(output) = std::process::Command::new("zsh")
        .arg("-c")
        .arg(script)
        .output()
    else {
        return;
    };

    // Check stderr for warnings/errors from .zshrc sourcing
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        eprintln!("\n❌ Error: Failed to source {}:", zshrc.display());
        eprintln!("{}", stderr);
        eprintln!("\n💡 Troubleshooting:");
        eprintln!("   1. Fix syntax errors in your .zshrc file");
        eprintln!("   2. OR skip .zshrc: export KODEGEN_SKIP_ZSHRC=1");
        eprintln!("   3. OR set env vars directly: export APPLE_CERTIFICATE=...\n");
        std::process::exit(1);
    }

    // Parse null-separated key-value pairs
    // Format: KEY1\0VALUE1\0KEY2\0VALUE2\0...
    let env_data = String::from_utf8_lossy(&output.stdout);
    let mut parts = env_data.split('\0');

    while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
        if !key.is_empty() {
            // SAFETY: This function is called as the FIRST operation in main(),
            // guaranteeing single-threaded execution. No threads exist yet.
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }
}

/// Async main logic - runs inside the Tokio runtime
async fn async_main() -> i32 {
    match cli::run().await {
        Ok(exit_code) => exit_code,
        Err(e) => {
            // Create output manager for error display (never quiet for fatal errors)
            let output = OutputManager::new(false, false);
            output.error(&format!("Fatal error: {e}"));

            // Show recovery suggestions for critical errors
            let suggestions = e.recovery_suggestions();
            if !suggestions.is_empty() {
                output.println("\n💡 Recovery suggestions:");
                for suggestion in suggestions {
                    output.indent(&suggestion);
                }
            }

            1
        }
    }
}

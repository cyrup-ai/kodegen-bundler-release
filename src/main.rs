//! Cyrup Release - Production-quality release management for Rust workspaces.
//!
//! This binary provides atomic release operations with proper error handling,
//! automatic internal dependency version synchronization, and rollback capabilities.

mod cli;
mod error;
mod git;
mod github;
mod metadata;
mod publish;
mod source;
mod state;
mod version;
mod workspace;

use cli::OutputManager;
use std::collections::HashMap;
use std::process;

/// Environment configuration that holds parsed .zshrc variables
/// and provides fallback to actual environment variables.
///
/// This struct eliminates the need for unsafe `std::env::set_var()` calls
/// by storing parsed values and providing safe access methods.
#[derive(Clone, Debug)]
pub struct EnvConfig {
    /// Variables parsed from .zshrc file
    zshrc_vars: HashMap<String, String>,
}

impl EnvConfig {
    /// Create new EnvConfig from parsed zshrc variables
    fn new(zshrc_vars: HashMap<String, String>) -> Self {
        Self { zshrc_vars }
    }

    /// Get environment variable value, checking zshrc vars first,
    /// then falling back to actual environment.
    pub fn get(&self, key: &str) -> Option<String> {
        self.zshrc_vars
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
    }

    /// Check if an environment variable is set (in zshrc or actual env)
    pub fn is_set(&self, key: &str) -> bool {
        self.zshrc_vars.contains_key(key) || std::env::var(key).is_ok()
    }
}

impl Default for EnvConfig {
    fn default() -> Self {
        Self {
            zshrc_vars: HashMap::new(),
        }
    }
}

fn main() {
    // Parse .zshrc environment variables (no unsafe set_var needed)
    let env_config = parse_zshrc_env_vars();

    // Safe to initialize logging
    env_logger::init();

    // Create tokio runtime
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to create Tokio runtime: {}", e);
            process::exit(1);
        }
    };

    // Run the async main logic with environment config
    let exit_code = runtime.block_on(async_main(env_config));
    process::exit(exit_code);
}

/// Parse ~/.zshrc and return environment variables as a HashMap.
///
/// This function safely parses the .zshrc file without using unsafe `std::env::set_var()`.
/// Variables are returned in a HashMap that can be queried via EnvConfig.
///
/// # Returns
/// EnvConfig containing parsed environment variables from .zshrc
fn parse_zshrc_env_vars() -> EnvConfig {
    // Allow skipping .zshrc sourcing if problematic
    // Useful for: CI environments, debugging, or when .zshrc has issues
    if std::env::var("KODEGEN_SKIP_ZSHRC").is_ok() {
        return EnvConfig::default();
    }
    
    // Source ~/.zshrc to load environment variables (APPLE_CERTIFICATE, etc.)
    // This is critical for code signing to work properly
    let Some(home) = dirs::home_dir() else {
        return EnvConfig::default();
    };

    let zshrc = home.join(".zshrc");
    if !zshrc.exists() {
        return EnvConfig::default();
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
        return EnvConfig::default();
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
    let mut zshrc_vars = HashMap::new();

    while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
        if !key.is_empty() {
            zshrc_vars.insert(key.to_string(), value.to_string());
        }
    }

    EnvConfig::new(zshrc_vars)
}

/// Async main logic - runs inside the Tokio runtime
async fn async_main(env_config: EnvConfig) -> i32 {
    match cli::run(env_config).await {
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

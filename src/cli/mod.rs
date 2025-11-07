//! Command line interface for cyrup_release.
//!
//! This module provides a comprehensive CLI for release management operations,
//! with proper argument parsing, command execution, and user feedback.

mod args;
pub mod commands;
mod output;
mod retry_config;

pub use args::{Args, RuntimeConfig};
pub use commands::execute_command;
pub use output::OutputManager;

use crate::error::Result;
use crate::EnvConfig;

/// Main CLI entry point
pub async fn run(env_config: EnvConfig) -> Result<i32> {
    let args = Args::parse_args();
    execute_command(args, env_config).await
}

/// Parse arguments without executing (for testing)
#[allow(dead_code)] // Public API - preserved for external consumers
pub fn parse_args() -> Args {
    Args::parse_args()
}

/// Validate arguments without executing (for testing)
#[allow(dead_code)] // Public API - preserved for external consumers
pub fn validate_args(args: &Args) -> std::result::Result<(), String> {
    args.validate()
}

/// Create runtime configuration from arguments
#[allow(dead_code)] // Public API - preserved for external consumers
pub fn create_runtime_config(_args: &Args) -> RuntimeConfig {
    RuntimeConfig::new()
}

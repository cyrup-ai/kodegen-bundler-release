//! Command line argument parsing and validation.
//!
//! This module provides minimal CLI argument parsing.
//! The tool is designed to "just work" - point it at a repo, it releases.

use clap::Parser;

/// Simple release tool for single Rust packages
#[derive(Parser, Debug)]
#[command(
    name = "kodegen_bundler_release",
    version,
    about = "Simple release tool for single Rust packages",
    long_about = "Create GitHub releases with multi-platform binary packages.

Usage:
  kodegen_bundler_release <source>
  kodegen_bundler_release cyrup-ai/kodegen-tools-filesystem
  kodegen_bundler_release /path/to/local/repo
  kodegen_bundler_release https://github.com/cyrup-ai/kodegen-tools-filesystem"
)]
pub struct Args {
    /// Repository source: local path, GitHub URL, or org/repo
    #[arg(index = 1, value_name = "SOURCE")]
    pub source: String,
}

impl Args {
    /// Parse command line arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Validate arguments for consistency
    pub fn validate(&self) -> Result<(), String> {
        // Validate source argument
        if self.source.is_empty() {
            return Err("Source repository is required".to_string());
        }

        Ok(())
    }
}

/// Configuration derived from command line arguments
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Output manager for colored terminal output
    output: super::OutputManager,
}

impl RuntimeConfig {
    /// Create runtime configuration
    pub fn new() -> Self {
        Self {
            output: super::OutputManager::new(false, false),
        }
    }

    /// Get a reference to the output manager
    pub fn output(&self) -> &super::OutputManager {
        &self.output
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeConfig {
    /// Print message
    pub fn println(&self, message: &str) {
        self.output.println(message);
    }

    /// Print verbose message (same as println - we always show everything)
    pub fn verbose_println(&self, message: &str) {
        self.output.println(message);
    }

    /// Print error message (always shown)
    pub fn error_println(&self, message: &str) {
        self.output.error(message);
    }

    /// Print warning message
    pub fn warning_println(&self, message: &str) {
        self.output.warn(message);
    }

    /// Print success message
    pub fn success_println(&self, message: &str) {
        self.output.success(message);
    }



    /// Print indented text
    pub fn indent(&self, message: &str) {
        self.output.indent(message);
    }

    /// Check if verbose output is enabled (always true now)
    pub fn is_verbose(&self) -> bool {
        true
    }
}

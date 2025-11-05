//! Command line argument parsing and validation.
//!
//! This module provides comprehensive CLI argument parsing using clap,
//! with proper validation and error handling.

use crate::version::VersionBump;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

/// Simple release tool for single Rust packages
#[derive(Parser, Debug)]
#[command(
    name = "kodegen_bundler_release",
    version,
    about = "Simple release tool for single Rust packages",
    long_about = "Create GitHub releases with multi-platform binary packages.

Usage:
  kodegen_bundler_release <source> [bump_type]
  kodegen_bundler_release cyrup-ai/kodegen-tools-filesystem
  kodegen_bundler_release cyrup-ai/kodegen-tools-filesystem minor
  kodegen_bundler_release /path/to/local/repo --dry-run"
)]
pub struct Args {
    // ===== POSITIONAL ARGUMENTS =====
    
    /// Repository source: local path, GitHub URL, or org/repo
    #[arg(index = 1, value_name = "SOURCE")]
    pub source: String,

    /// Version bump type: patch, minor, major
    #[arg(index = 2, value_enum, default_value = "patch")]
    pub bump_type: BumpType,

    // ===== RELEASE FLAGS =====

    /// Perform dry run without making changes
    #[arg(short, long)]
    pub dry_run: bool,

    /// Skip validation checks
    #[arg(long)]
    pub skip_validation: bool,

    /// Force release even if working directory is dirty
    #[arg(long)]
    pub allow_dirty: bool,

    /// Don't push to remote repository
    #[arg(long)]
    pub no_push: bool,

    /// Registry to publish to (defaults to crates.io)
    #[arg(long, value_name = "REGISTRY")]
    pub registry: Option<String>,

    /// GitHub repository override (format: owner/repo)
    #[arg(long, value_name = "OWNER/REPO")]
    pub github_repo: Option<String>,

    /// Create universal binaries for macOS (x86_64 + arm64)
    #[arg(long)]
    pub universal: bool,

    // ===== GLOBAL FLAGS =====

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress all output except errors
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Path to workspace root (defaults to current directory)
    #[arg(short, long, global = true, value_name = "PATH")]
    pub workspace: Option<PathBuf>,

    /// Path to state file
    #[arg(long, global = true, value_name = "PATH")]
    pub state_file: Option<PathBuf>,

    /// Configuration file path
    #[arg(short, long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    // ===== DOCKER CONTAINER LIMITS =====

    /// Docker container memory limit (e.g., "2g", "4096m")
    /// 
    /// Defaults to auto-detected safe limit (50% of host RAM, min 2GB, max 16GB)
    #[arg(long, env = "KODEGEN_DOCKER_MEMORY")]
    pub docker_memory: Option<String>,

    /// Docker container memory + swap limit (e.g., "6g", "8192m")
    /// 
    /// Must be ≥ memory limit. Defaults to memory + 2GB if not specified.
    #[arg(long, env = "KODEGEN_DOCKER_MEMORY_SWAP")]
    pub docker_memory_swap: Option<String>,

    /// Docker container CPU limit (e.g., "2.0", "4", "1.5")
    /// 
    /// Supports fractional values. Defaults to auto-detected (50% of host cores, min 2)
    #[arg(long, env = "KODEGEN_DOCKER_CPUS")]
    pub docker_cpus: Option<String>,

    /// Docker container process limit
    /// 
    /// Maximum number of processes. Defaults to 1000.
    #[arg(long, env = "KODEGEN_DOCKER_PIDS_LIMIT")]
    pub docker_pids_limit: Option<u32>,
}



/// Type of version bump
#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
pub enum BumpType {
    /// Bump major version (breaking changes)
    Major,
    /// Bump minor version (new features)
    Minor,
    /// Bump patch version (bug fixes)
    Patch,
    /// Set exact version
    Exact,
}



impl TryFrom<BumpType> for VersionBump {
    type Error = String;

    fn try_from(bump_type: BumpType) -> Result<Self, Self::Error> {
        match bump_type {
            BumpType::Major => Ok(VersionBump::Major),
            BumpType::Minor => Ok(VersionBump::Minor),
            BumpType::Patch => Ok(VersionBump::Patch),
            BumpType::Exact => Err(
                "Exact version bump requires --version parameter (not yet implemented)".to_string(),
            ),
        }
    }
}

impl Args {
    /// Parse command line arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Get workspace path or default to current directory
    pub fn workspace_path(&self) -> PathBuf {
        self.workspace.clone().unwrap_or_else(|| PathBuf::from("."))
    }

    /// Get state file path or default
    pub fn state_file_path(&self) -> PathBuf {
        self.state_file
            .clone()
            .unwrap_or_else(|| PathBuf::from(".cyrup_release_state.json"))
    }

    /// Check if running in verbose mode
    pub fn is_verbose(&self) -> bool {
        self.verbose && !self.quiet
    }

    /// Check if running in quiet mode
    pub fn is_quiet(&self) -> bool {
        self.quiet
    }

    /// Validate arguments for consistency
    pub fn validate(&self) -> Result<(), String> {
        // Check for conflicting global options
        if self.verbose && self.quiet {
            return Err("Cannot specify both --verbose and --quiet".to_string());
        }

        // Validate source argument
        if self.source.is_empty() {
            return Err("Source repository is required".to_string());
        }

        // Validate workspace path if provided
        if let Some(ref workspace) = self.workspace {
            if !workspace.exists() {
                return Err(format!(
                    "Workspace path does not exist: {}",
                    workspace.display()
                ));
            }
            if !workspace.is_dir() {
                return Err(format!(
                    "Workspace path is not a directory: {}",
                    workspace.display()
                ));
            }
        }

        // Validate state file path if provided
        if let Some(ref state_file) = self.state_file
            && let Some(parent) = state_file.parent()
            && !parent.exists()
        {
            return Err(format!(
                "State file directory does not exist: {}",
                parent.display()
            ));
        }

        Ok(())
    }
}



/// Configuration derived from command line arguments
#[derive(Debug)]
pub struct RuntimeConfig {
    /// Workspace root path
    pub workspace_path: PathBuf,
    /// State file path
    pub state_file_path: PathBuf,
    /// Verbosity level
    pub verbosity: VerbosityLevel,
    /// Registry to use
    pub registry: Option<String>,
    /// Output manager for colored terminal output
    output: super::OutputManager,
    /// Docker container memory limit
    pub docker_memory: Option<String>,
    /// Docker container memory + swap limit  
    pub docker_memory_swap: Option<String>,
    /// Docker container CPU limit
    pub docker_cpus: Option<String>,
    /// Docker container process limit
    pub docker_pids_limit: Option<u32>,
}

/// Verbosity level for output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbosityLevel {
    /// Minimal output
    Quiet,
    /// Standard output
    Normal,
    /// Detailed output
    Verbose,
}

impl From<&Args> for RuntimeConfig {
    fn from(args: &Args) -> Self {
        let verbosity = if args.quiet {
            VerbosityLevel::Quiet
        } else if args.verbose {
            VerbosityLevel::Verbose
        } else {
            VerbosityLevel::Normal
        };

        let output = super::OutputManager::new(
            verbosity == VerbosityLevel::Verbose,
            verbosity == VerbosityLevel::Quiet,
        );

        Self {
            workspace_path: args.workspace_path(),
            state_file_path: args.state_file_path(),
            verbosity,
            registry: args.registry.clone(),
            output,
            // Docker container limits
            docker_memory: args.docker_memory.clone(),
            docker_memory_swap: args.docker_memory_swap.clone(),
            docker_cpus: args.docker_cpus.clone(),
            docker_pids_limit: args.docker_pids_limit,
        }
    }
}

impl RuntimeConfig {
    /// Check if output should be suppressed
    pub fn is_quiet(&self) -> bool {
        self.verbosity == VerbosityLevel::Quiet
    }

    /// Check if verbose output is enabled
    pub fn is_verbose(&self) -> bool {
        self.verbosity == VerbosityLevel::Verbose
    }

    /// Print message if not in quiet mode
    pub fn println(&self, message: &str) {
        self.output.println(message);
    }

    /// Print verbose message if in verbose mode
    pub fn verbose_println(&self, message: &str) {
        self.output.verbose(message);
    }

    /// Print error message (always shown)
    pub fn error_println(&self, message: &str) {
        self.output.error(message);
    }

    /// Print warning message if not in quiet mode
    pub fn warning_println(&self, message: &str) {
        self.output.warn(message);
    }

    /// Print success message if not in quiet mode
    pub fn success_println(&self, message: &str) {
        self.output.success(message);
    }

    /// Print success message (alias for success_println for convenience)
    pub fn success(&self, message: &str) {
        self.output.success(message);
    }

    /// Print warning message (alias for warning_println for convenience)
    pub fn warn(&self, message: &str) {
        self.output.warn(message);
    }

    /// Print error message (always shown, alias for error_println)
    pub fn error(&self, message: &str) {
        self.output.error(message);
    }

    /// Print info message
    pub fn info(&self, message: &str) {
        self.output.info(message);
    }

    /// Print progress message
    pub fn progress(&self, message: &str) {
        self.output.progress(message);
    }

    /// Print section header
    pub fn section(&self, title: &str) {
        self.output.section(title);
    }

    /// Print indented text
    pub fn indent(&self, message: &str) {
        self.output.indent(message);
    }
}

//! Retry configuration for network operations.
//!
//! Provides configurable retry limits for different operation types,
//! allowing users to tune retry behavior based on network conditions.

#![allow(dead_code)] // Public API - may be used by external consumers

/// Configuration for retry behavior across different operation types
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Max retries for git operations (commit, tag, push)
    pub git_operations: u32,
    
    /// Max retries for GitHub API calls (create release, etc.)
    pub github_api: u32,
    
    /// Max retries for file upload operations
    pub file_uploads: u32,
    
    /// Max retries for release publishing (both GitHub and crates.io)
    pub release_publishing: u32,
    
    /// Max retries for cleanup operations (deletion, rollback)
    pub cleanup_operations: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            git_operations: 3,        // Conservative - git is deterministic
            github_api: 5,            // Higher - network + rate limits
            file_uploads: 5,          // Higher - most network-dependent
            release_publishing: 3,    // Conservative - idempotent operation
            cleanup_operations: 3,    // Conservative - best-effort cleanup
        }
    }
}

impl RetryConfig {
    /// Parse retry count from environment variable with clamping to maximum
    ///
    /// # Arguments
    /// * `var_name` - Environment variable name (e.g., "KODEGEN_RETRY_GIT")
    /// * `default` - Default value if variable is not set or invalid
    /// * `max` - Maximum allowed value (values above this are clamped)
    ///
    /// # Returns
    /// Retry count clamped to [0, max]
    fn parse_retry_env(var_name: &str, default: u32, max: u32) -> u32 {
        std::env::var(var_name)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|v| v.min(max))  // Clamp to max
            .unwrap_or(default)
    }
    
    /// Create config from environment variables with fallback to defaults
    pub fn from_env() -> Self {
        Self {
            git_operations: Self::parse_retry_env("KODEGEN_RETRY_GIT", 3, 10),
            github_api: Self::parse_retry_env("KODEGEN_RETRY_GITHUB", 5, 20),
            file_uploads: Self::parse_retry_env("KODEGEN_RETRY_UPLOADS", 5, 20),
            release_publishing: Self::parse_retry_env("KODEGEN_RETRY_PUBLISH", 3, 10),
            cleanup_operations: Self::parse_retry_env("KODEGEN_RETRY_CLEANUP", 3, 10),
        }
    }
    
    /// Validate retry counts are reasonable
    pub fn validate(&self) -> Result<(), String> {
        if self.git_operations > 10 {
            return Err(format!(
                "git_operations retry count too high: {} (max: 10)",
                self.git_operations
            ));
        }
        if self.github_api > 20 {
            return Err(format!(
                "github_api retry count too high: {} (max: 20)",
                self.github_api
            ));
        }
        if self.file_uploads > 20 {
            return Err(format!(
                "file_uploads retry count too high: {} (max: 20)",
                self.file_uploads
            ));
        }
        if self.release_publishing > 10 {
            return Err(format!(
                "release_publishing retry count too high: {} (max: 10)",
                self.release_publishing
            ));
        }
        if self.cleanup_operations > 10 {
            return Err(format!(
                "cleanup_operations retry count too high: {} (max: 10)",
                self.cleanup_operations
            ));
        }
        Ok(())
    }
}

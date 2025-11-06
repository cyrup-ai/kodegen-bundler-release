//! Shared helper functions for command execution.

use crate::error::{CliError, ReleaseError, Result};
use crate::git::GitManager;
use gix::bstr::{BStr, ByteSlice};

/// Parse GitHub repository string into owner/repo tuple
#[allow(dead_code)]
pub(super) fn parse_github_repo(repo_str: Option<&str>) -> Result<(String, String)> {
    let repo = repo_str.ok_or_else(|| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason: "--github-repo is required when --github-release is used. Format: owner/repo"
                .to_string(),
        })
    })?;

    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Invalid GitHub repository format: '{}'. Expected: owner/repo",
                repo
            ),
        }));
    }

    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Parse GitHub repo string "owner/repo"
pub(super) fn parse_github_repo_string(repo_str: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = repo_str.split('/').collect();
    if parts.len() != 2 {
        return Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Invalid GitHub repository format: '{}'. Expected: owner/repo",
                repo_str
            ),
        }));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Parse GitHub owner/repo from git remote URL using proper URL parsing
/// 
/// Supports all Git URL formats:
/// - SSH SCP-like: git@github.com:owner/repo.git
/// - SSH protocol: ssh://git@github.com/owner/repo.git
/// - HTTPS: https://github.com/owner/repo.git
/// - HTTP: http://github.com/owner/repo.git
/// - With ports: ssh://git@github.com:2222/owner/repo.git
/// - Enterprise: git@github.company.com:owner/repo.git
/// 
/// Returns (owner, repo) tuple or error with context about what failed.
pub(super) fn parse_github_url(url: &str) -> Result<(String, String)> {
    // Parse URL using gix-url for robust handling of all Git URL formats
    let parsed_url = gix_url::parse(BStr::new(url.as_bytes()))
        .map_err(|e| ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!("Failed to parse Git URL '{}': {}", url, e),
        }))?;
    
    // Extract path component (e.g., "/owner/repo.git" or "owner/repo.git")
    let path = parsed_url.path.to_str_lossy();
    
    // Remove leading slash if present
    let path = path.strip_prefix('/').unwrap_or(&path);
    
    // Remove .git suffix if present
    let path = path.strip_suffix(".git").unwrap_or(path);
    
    // Split by / and extract owner/repo
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    
    if parts.len() < 2 {
        return Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Git URL does not contain owner/repo path: '{}' (parsed path: '{}')",
                url, path
            ),
        }));
    }
    
    // Take first two non-empty segments as owner/repo
    // This handles both "owner/repo" and "owner/subgroup/repo" cases
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Detect GitHub repo from git remote origin using GitManager
pub(super) async fn detect_github_repo(git_manager: &GitManager) -> Result<(String, String)> {
    let remotes = git_manager.remotes().await?;

    // Find origin remote
    let origin = remotes.iter().find(|r| r.name == "origin").ok_or_else(|| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason:
                "No 'origin' remote configured. Git requires origin for push/pull/tag operations."
                    .to_string(),
        })
    })?;

    // Parse GitHub URL from origin
    parse_github_url(&origin.fetch_url)
}




/// Prompt user for confirmation with y/n input
/// 
/// Returns true if user confirms (y/yes), false if user declines (n/no/empty)
/// 
/// # Arguments
/// * `prompt` - The question to ask (without [y/N] suffix)
/// 
/// # Example
/// ```
/// if !prompt_confirmation("About to delete files")? {
///     println!("Operation cancelled");
///     return Ok(());
/// }
/// ```
#[allow(dead_code)]
pub(super) fn prompt_confirmation(prompt: &str) -> std::io::Result<bool> {
    use std::io::Write;
    
    print!("{} [y/N]: ", prompt);
    std::io::stdout().flush()?;
    
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    
    let response = input.trim().to_lowercase();
    Ok(matches!(response.as_str(), "y" | "yes"))
}











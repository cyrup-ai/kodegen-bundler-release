//! Shared helper functions for command execution.

use crate::error::{CliError, ReleaseError, Result};
use crate::git::GitManager;
use std::path::Path;

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

/// Parse GitHub owner/repo from git remote URL
/// Supports: git@github.com:owner/repo.git and https://github.com/owner/repo.git
pub(super) fn parse_github_url(url: &str) -> Option<(String, String)> {
    // Handle git@github.com:owner/repo.git (with or without leading slash)
    if let Some(ssh_part) = url.strip_prefix("git@github.com:") {
        // Remove leading slash if present (malformed URL like git@github.com:/owner/repo)
        let ssh_part = ssh_part.strip_prefix('/').unwrap_or(ssh_part);
        let repo_part = ssh_part.strip_suffix(".git").unwrap_or(ssh_part);
        let parts: Vec<&str> = repo_part.split('/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // Handle https://github.com/owner/repo.git
    if url.contains("github.com/")
        && let Some(path) = url.split("github.com/").nth(1)
    {
        let repo_part = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<&str> = repo_part.split('/').collect();
        if parts.len() >= 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    None
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
    parse_github_url(&origin.fetch_url).ok_or_else(|| {
        ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Origin remote is not a GitHub repository: {}",
                origin.fetch_url
            ),
        })
    })
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











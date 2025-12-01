//! Shared helper functions for command execution.

use crate::error::{CliError, ReleaseError, Result};

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

/// Parse GitHub owner/repo from git remote URL
///
/// Supports Git URL formats:
/// - SSH SCP-like: git@github.com:owner/repo.git
/// - HTTPS: https://github.com/owner/repo.git
pub(super) fn parse_github_url(url: &str) -> Result<(String, String)> {
    // Handle SSH SCP-like format: git@github.com:owner/repo.git
    if url.contains('@') && url.contains(':') && !url.contains("://") {
        let parts: Vec<&str> = url.split(':').collect();
        if parts.len() == 2 {
            let path = parts[1].trim_end_matches(".git");
            let path_parts: Vec<&str> = path.split('/').collect();
            if path_parts.len() == 2 {
                return Ok((path_parts[0].to_string(), path_parts[1].to_string()));
            }
        }
    }

    // Handle HTTPS/SSH protocol URLs
    if let Some(path_start) = url.find("github.com/") {
        let path = &url[path_start + 11..];
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    Err(ReleaseError::Cli(CliError::InvalidArguments {
        reason: format!("Could not parse GitHub owner/repo from URL: '{}'", url),
    }))
}

/// Prompt user for confirmation with y/n input
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

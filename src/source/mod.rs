//! Repository source resolution (local paths, GitHub URLs, org/repo notation)

use crate::error::{CliError, ReleaseError, Result};
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Repository source: local path or GitHub
pub enum RepositorySource {
    Local(PathBuf),
    GitHub { owner: String, repo: String },
}

impl RepositorySource {
    /// Parse input string into RepositorySource
    pub fn parse(input: &str) -> Result<Self> {
        // Try as local path first
        let path = PathBuf::from(input);
        if path.exists() {
            return path.canonicalize().map(Self::Local).map_err(|e| {
                ReleaseError::Cli(CliError::InvalidArguments {
                    reason: format!("Failed to canonicalize path '{}': {}", input, e),
                })
            });
        }

        // Try as GitHub URL: https://github.com/owner/repo
        static GITHUB_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"https://github\.com/(?P<owner>[^/]+)/(?P<repo>[^/.]+)")
                .expect("GitHub URL regex is valid")
        });

        if let Some(caps) = GITHUB_URL_RE.captures(input) {
            return Ok(Self::GitHub {
                owner: caps["owner"].to_string(),
                repo: caps["repo"].to_string(),
            });
        }

        // Try as org/repo notation
        if let Some((owner, repo)) = input.split_once('/') {
            return Ok(Self::GitHub {
                owner: owner.to_string(),
                repo: repo.trim_end_matches(".git").to_string(),
            });
        }

        Err(ReleaseError::Cli(CliError::InvalidArguments {
            reason: format!(
                "Invalid source: '{}'. Use local path, GitHub URL, or org/repo",
                input
            ),
        }))
    }

    /// Resolve to local path (clone if GitHub)
    pub async fn resolve(&self) -> Result<ResolvedRepo> {
        match self {
            Self::Local(path) => Ok(ResolvedRepo {
                path: path.clone(),
                is_temp: false,
            }),
            Self::GitHub { owner, repo } => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| {
                        ReleaseError::Cli(CliError::ExecutionFailed {
                            command: "get_timestamp".to_string(),
                            reason: e.to_string(),
                        })
                    })?
                    .as_secs();

                let temp_dir = std::env::temp_dir().join(format!("kodegen-release-{}", timestamp));
                let remote_url = format!("git@github.com:{}/{}.git", owner, repo);

                // Clone using git command
                let output = tokio::process::Command::new("git")
                    .args([
                        "clone",
                        "--single-branch",
                        &remote_url,
                        temp_dir.to_str().unwrap(),
                    ])
                    .output()
                    .await
                    .map_err(|e| {
                        ReleaseError::Cli(CliError::ExecutionFailed {
                            command: "git clone".to_string(),
                            reason: e.to_string(),
                        })
                    })?;

                if !output.status.success() {
                    return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                        command: "git clone".to_string(),
                        reason: String::from_utf8_lossy(&output.stderr).to_string(),
                    }));
                }

                Ok(ResolvedRepo {
                    path: temp_dir,
                    is_temp: true,
                })
            }
        }
    }
}

/// Resolved repository with automatic cleanup
pub struct ResolvedRepo {
    pub path: PathBuf,
    pub is_temp: bool,
}

impl Drop for ResolvedRepo {
    fn drop(&mut self) {
        if !self.is_temp {
            return;
        }

        const MAX_RETRIES: u32 = 3;
        const RETRY_DELAY_MS: u64 = 100;

        for attempt in 0..MAX_RETRIES {
            match std::fs::remove_dir_all(&self.path) {
                Ok(()) => return,
                Err(_e) if attempt < MAX_RETRIES - 1 => {
                    std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAY_MS));
                }
                Err(e) => {
                    eprintln!(
                        "⚠ Warning: Failed to cleanup temporary repository.\n\
                         Path: {}\n\
                         Error: {}",
                        self.path.display(),
                        e
                    );
                }
            }
        }
    }
}

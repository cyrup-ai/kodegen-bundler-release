//! Temp clone helpers for isolated release execution.
//!
//! This module provides functionality to clone the repository to a temporary
//! directory for isolated release operations, and track active temp paths.

use crate::error::{CliError, ReleaseError, Result};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use sysinfo::{System, Pid};

/// Get origin URL from current workspace for cloning
pub(super) async fn get_origin_url_for_clone(workspace_path: &std::path::Path) -> Result<String> {
    let repo = kodegen_tools_git::discover_repo(workspace_path)
        .await
        .map_err(|_| ReleaseError::Git(crate::error::GitError::NotRepository))?
        .map_err(|_| ReleaseError::Git(crate::error::GitError::NotRepository))?;

    let remotes = kodegen_tools_git::list_remotes(&repo).await.map_err(|e| {
        ReleaseError::Git(crate::error::GitError::RemoteOperationFailed {
            operation: "list_remotes".to_string(),
            reason: e.to_string(),
        })
    })?;
    let origin = remotes.iter().find(|r| r.name == "origin").ok_or_else(|| {
        ReleaseError::Git(crate::error::GitError::RemoteOperationFailed {
            operation: "find_origin".to_string(),
            reason: "No origin remote found. Please configure an 'origin' remote.".to_string(),
        })
    })?;

    Ok(origin.fetch_url.clone())
}

/// Clone main branch to temporary directory for isolated release execution
pub(super) async fn clone_main_to_temp_for_release(
    workspace_path: &std::path::Path,
) -> Result<PathBuf> {
    // Get origin URL from current repository
    let remote_url = get_origin_url_for_clone(workspace_path).await?;

    // Create unique temp directory with timestamp
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

    // Clone main branch to temp
    let clone_opts = kodegen_tools_git::CloneOpts::new(remote_url, temp_dir.clone()).branch("main");

    kodegen_tools_git::clone_repo(clone_opts)
        .await
        .map_err(|e| {
            ReleaseError::Git(crate::error::GitError::RemoteOperationFailed {
                operation: "clone_repo".to_string(),
                reason: format!("Failed to clone repository: {}", e),
            })
        })?
        .map_err(|e| {
            ReleaseError::Git(crate::error::GitError::RemoteOperationFailed {
                operation: "clone_repo".to_string(),
                reason: e.to_string(),
            })
        })?;

    // Track temp path for recovery
    save_active_temp_path(&temp_dir)?;

    Ok(temp_dir)
}

/// Metadata for tracking an active release process
///
/// Each release process creates one tracking file at:
/// `~/.kodegen/active_releases/{pid}.json`
///
/// This enables:
/// - Concurrent releases (each process has unique PID)
/// - Stale cleanup (check if PID still alive)
/// - Resume support (find current process's release)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReleaseTracking {
    /// Process ID that created this release
    pid: u32,
    
    /// Absolute path to temporary clone directory
    temp_path: PathBuf,
    
    /// ISO 8601 timestamp when release started
    started_at: String,
    
    /// Project name being released
    project: String,
    
    /// Target version for this release
    version: String,
}

/// Save active temp release path with metadata for current process
///
/// Creates tracking file at `~/.kodegen/active_releases/{pid}.json`
/// enabling concurrent releases and automatic stale cleanup.
pub(super) fn save_active_temp_path(temp_dir: &std::path::Path) -> Result<()> {
    let config_dir = dirs::home_dir()
        .ok_or_else(|| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "get_home_dir".to_string(),
                reason: "Could not determine home directory".to_string(),
            })
        })?
        .join(".kodegen")
        .join("active_releases");  // ✅ NEW: Subdirectory for PID files

    std::fs::create_dir_all(&config_dir).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "create_tracking_dir".to_string(),
            reason: e.to_string(),
        })
    })?;

    // ✅ NEW: PID-based filename
    let pid = std::process::id();
    let tracking_file = config_dir.join(format!("{}.json", pid));
    
    // ✅ NEW: Rich metadata (extract from context if available, use defaults)
    let tracking = ReleaseTracking {
        pid,
        temp_path: temp_dir.to_path_buf(),
        started_at: chrono::Utc::now().to_rfc3339(),
        project: temp_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string(),
        version: "unknown".to_string(),  // Can be enhanced later
    };
    
    let json = serde_json::to_string_pretty(&tracking).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "serialize_tracking".to_string(),
            reason: e.to_string(),
        })
    })?;
    
    std::fs::write(&tracking_file, json).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "save_temp_path".to_string(),
            reason: e.to_string(),
        })
    })?;

    Ok(())
}

/// Get active temp release path for current process if one exists
///
/// Returns:
/// - `Some(path)` if current PID has valid tracking and path exists
/// - `None` if no tracking or path no longer exists (stale)
///
/// Automatically cleans up stale tracking if path doesn't exist.
#[allow(dead_code)]
pub(super) fn get_active_temp_path() -> Option<PathBuf> {
    let config_dir = dirs::home_dir()?
        .join(".kodegen")
        .join("active_releases");
    
    if !config_dir.exists() {
        return None;
    }
    
    // ✅ NEW: Read current PID's tracking file
    let current_pid = std::process::id();
    let tracking_file = config_dir.join(format!("{}.json", current_pid));
    
    if !tracking_file.exists() {
        return None;
    }
    
    // Parse tracking metadata
    let content = std::fs::read_to_string(&tracking_file).ok()?;
    let tracking: ReleaseTracking = serde_json::from_str(&content).ok()?;
    
    // ✅ NEW: Verify temp path still exists
    if !tracking.temp_path.exists() {
        // Stale tracking - clean it up
        let _ = std::fs::remove_file(&tracking_file);
        return None;
    }
    
    Some(tracking.temp_path)
}

/// Clear active temp release path tracking for current process
///
/// Removes `~/.kodegen/active_releases/{pid}.json` for current PID.
/// Safe to call multiple times (idempotent).
pub(super) fn clear_active_temp_path() -> Result<()> {
    if let Some(home_dir) = dirs::home_dir() {
        let current_pid = std::process::id();
        let tracking_file = home_dir
            .join(".kodegen")
            .join("active_releases")
            .join(format!("{}.json", current_pid));  // ✅ NEW: PID-based
        
        if tracking_file.exists() {
            std::fs::remove_file(&tracking_file).map_err(|e| {
                ReleaseError::Cli(CliError::ExecutionFailed {
                    command: "clear_temp_path".to_string(),
                    reason: e.to_string(),
                })
            })?;
        }
    }
    Ok(())
}

/// Clean up stale tracking files from dead processes
///
/// Scans `~/.kodegen/active_releases/` for tracking files whose
/// processes no longer exist, then removes both the tracking file
/// and the temp directory (if it still exists).
///
/// Should be called at the start of each release to clean up
/// crashed previous releases.
///
/// Returns:
/// - `Ok(usize)` - Number of stale releases cleaned up
/// - `Err(_)` - If cleanup operations fail critically
pub(super) fn cleanup_stale_tracking() -> Result<usize> {
    let config_dir = match dirs::home_dir() {
        Some(home) => home.join(".kodegen").join("active_releases"),
        None => return Ok(0),  // Can't determine home, skip cleanup
    };
    
    if !config_dir.exists() {
        return Ok(0);  // No tracking directory, nothing to clean
    }
    
    // ✅ NEW: Initialize sysinfo for process checking
    let mut sys = System::new_all();
    sys.refresh_all();
    
    let mut cleaned_count = 0;
    
    // Iterate all tracking files
    for entry in std::fs::read_dir(&config_dir).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "read_tracking_dir".to_string(),
            reason: e.to_string(),
        })
    })? {
        let entry = entry.map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "read_tracking_entry".to_string(),
                reason: e.to_string(),
            })
        })?;
        
        let path = entry.path();
        
        // Only process .json files
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        
        // Try to parse tracking file
        let tracking: ReleaseTracking = match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(t) => t,
                Err(_) => {
                    // Invalid JSON - clean up corrupted file
                    let _ = std::fs::remove_file(&path);
                    cleaned_count += 1;
                    continue;
                }
            },
            Err(_) => {
                // Can't read - clean up
                let _ = std::fs::remove_file(&path);
                cleaned_count += 1;
                continue;
            }
        };
        
        // ✅ NEW: Check if process is still alive using sysinfo
        let pid = Pid::from_u32(tracking.pid);
        if sys.process(pid).is_none() {
            // Process dead - clean up tracking file
            let _ = std::fs::remove_file(&path);
            
            // Also clean up temp directory if it exists
            if tracking.temp_path.exists() {
                let _ = std::fs::remove_dir_all(&tracking.temp_path);
            }
            
            cleaned_count += 1;
        }
    }
    
    Ok(cleaned_count)
}

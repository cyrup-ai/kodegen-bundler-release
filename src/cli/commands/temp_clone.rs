//! Temp clone helpers for isolated release execution.

use crate::error::{CliError, ReleaseError, Result};
use kodegen_config::KodegenConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use sysinfo::{Pid, System};

/// Get origin URL from current workspace for cloning
pub(super) async fn get_origin_url_for_clone(workspace_path: &std::path::Path) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(workspace_path)
        .output()
        .await
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "git remote get-url origin".to_string(),
                reason: e.to_string(),
            })
        })?;

    if !output.status.success() {
        return Err(ReleaseError::Cli(CliError::ExecutionFailed {
            command: "git remote get-url origin".to_string(),
            reason: String::from_utf8_lossy(&output.stderr).to_string(),
        }));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Clone main branch to temporary directory for isolated release execution
pub(super) async fn clone_main_to_temp_for_release(
    workspace_path: &std::path::Path,
) -> Result<PathBuf> {
    let remote_url = get_origin_url_for_clone(workspace_path).await?;

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

    // Clone using git command
    let output = tokio::process::Command::new("git")
        .args([
            "clone",
            "--branch",
            "main",
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

    save_active_temp_path(&temp_dir)?;

    Ok(temp_dir)
}

/// Metadata for tracking an active release process
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReleaseTracking {
    pid: u32,
    temp_path: PathBuf,
    started_at: String,
    project: String,
    version: String,
}

/// Save active temp release path with metadata for current process
pub(super) fn save_active_temp_path(temp_dir: &std::path::Path) -> Result<()> {
    let config_dir = KodegenConfig::state_dir()
        .map(|dir| dir.join("active_releases"))
        .map_err(|e| {
            ReleaseError::Cli(CliError::ExecutionFailed {
                command: "get_state_dir".to_string(),
                reason: e.to_string(),
            })
        })?;

    std::fs::create_dir_all(&config_dir).map_err(|e| {
        ReleaseError::Cli(CliError::ExecutionFailed {
            command: "create_tracking_dir".to_string(),
            reason: e.to_string(),
        })
    })?;

    let pid = std::process::id();
    let tracking_file = config_dir.join(format!("{}.json", pid));

    let tracking = ReleaseTracking {
        pid,
        temp_path: temp_dir.to_path_buf(),
        started_at: chrono::Utc::now().to_rfc3339(),
        project: temp_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string(),
        version: "unknown".to_string(),
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
#[allow(dead_code)]
pub(super) fn get_active_temp_path() -> Option<PathBuf> {
    let config_dir = KodegenConfig::state_dir().ok()?.join("active_releases");

    if !config_dir.exists() {
        return None;
    }

    let current_pid = std::process::id();
    let tracking_file = config_dir.join(format!("{}.json", current_pid));

    if !tracking_file.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&tracking_file).ok()?;
    let tracking: ReleaseTracking = serde_json::from_str(&content).ok()?;

    if !tracking.temp_path.exists() {
        let _ = std::fs::remove_file(&tracking_file);
        return None;
    }

    Some(tracking.temp_path)
}

/// Clear active temp release path tracking for current process
pub(super) fn clear_active_temp_path() -> Result<()> {
    if let Ok(state_dir) = KodegenConfig::state_dir() {
        let current_pid = std::process::id();
        let tracking_file = state_dir
            .join("active_releases")
            .join(format!("{}.json", current_pid));

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
pub(super) fn cleanup_stale_tracking() -> Result<usize> {
    let config_dir = match KodegenConfig::state_dir() {
        Ok(state) => state.join("active_releases"),
        Err(_) => return Ok(0),
    };

    if !config_dir.exists() {
        return Ok(0);
    }

    let mut sys = System::new_all();
    sys.refresh_all();

    let mut cleaned_count = 0;

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

        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let tracking: ReleaseTracking = match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(t) => t,
                Err(_) => {
                    let _ = std::fs::remove_file(&path);
                    cleaned_count += 1;
                    continue;
                }
            },
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                cleaned_count += 1;
                continue;
            }
        };

        let pid = Pid::from_u32(tracking.pid);
        if sys.process(pid).is_none() {
            let _ = std::fs::remove_file(&path);

            if tracking.temp_path.exists() {
                let _ = std::fs::remove_dir_all(&tracking.temp_path);
            }

            cleaned_count += 1;
        }
    }

    Ok(cleaned_count)
}

//! State persistence and management for release operations.
//!
//! This module provides robust state persistence with file locking,
//! corruption recovery, and atomic operations.

use crate::error::{Result, StateError};
use crate::state::ReleaseState;
use serde_json;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// State manager for persistent release state
#[derive(Debug)]
pub struct StateManager {
    /// Path to state file
    state_file_path: PathBuf,
    /// Path to lock file
    lock_file_path: PathBuf,
    /// Current lock handle
    lock_handle: Option<FileLock>,
    /// Configuration for state management
    config: StateConfig,
}

/// Configuration for state management
#[derive(Debug, Clone)]
pub struct StateConfig {
    /// Maximum age of state files before cleanup (in seconds)
    pub max_state_age_seconds: u64,
    /// Whether to compress state files
    pub compress_state: bool,
    /// Timeout for acquiring file locks (in milliseconds)
    pub lock_timeout_ms: u64,
    /// Whether to validate state on load
    pub validate_on_load: bool,
    /// Timeout for considering a lock stale (in seconds)
    pub stale_lock_timeout_seconds: u64,
}

impl Default for StateConfig {
    fn default() -> Self {
        Self {
            max_state_age_seconds: 86400 * 7, // 7 days
            compress_state: false,
            lock_timeout_ms: 5000, // 5 seconds
            validate_on_load: true,
            stale_lock_timeout_seconds: 3600, // 1 hour
        }
    }
}

/// File lock implementation with advisory lock handle
#[derive(Debug)]
struct FileLock {
    /// Path to lock file
    _lock_file: PathBuf,
    /// Process ID of the locking process
    _pid: u32,
    /// Timestamp when lock was acquired
    _acquired_at: SystemTime,
    /// File handle that holds the flock
    /// CRITICAL: Must be kept alive for lock to remain held.
    /// The flock is automatically released when this file handle is dropped.
    /// See Drop implementation at line 580.
    _lock_handle: std::fs::File,
}

/// Result of state loading operation
#[derive(Debug)]
pub struct LoadStateResult {
    /// Loaded release state
    pub state: ReleaseState,
    /// Whether state was recovered from backup
    pub recovered_from_backup: bool,
    /// Any warnings during loading
    pub warnings: Vec<String>,
}

/// Result of state saving operation
#[derive(Debug)]
pub struct SaveStateResult {
    /// Whether save was successful
    pub success: bool,
    /// Size of saved state file in bytes
    pub file_size_bytes: u64,
    /// Duration of save operation
    pub save_duration: Duration,
}

impl StateManager {
    /// Create a new state manager
    pub fn new<P: AsRef<Path>>(state_file_path: P) -> Result<Self> {
        let state_file_path = state_file_path.as_ref().to_path_buf();
        let lock_file_path = state_file_path.with_extension("lock");

        Ok(Self {
            state_file_path,
            lock_file_path,
            lock_handle: None,
            config: StateConfig::default(),
        })
    }

    /// Create a state manager with custom configuration
    pub fn with_config<P: AsRef<Path>>(state_file_path: P, config: StateConfig) -> Result<Self> {
        let state_file_path = state_file_path.as_ref().to_path_buf();
        let lock_file_path = state_file_path.with_extension("lock");

        Ok(Self {
            state_file_path,
            lock_file_path,
            lock_handle: None,
            config,
        })
    }

    /// Save release state to file
    pub async fn save_state(&mut self, state: &ReleaseState) -> Result<SaveStateResult> {
        let start_time = SystemTime::now();

        // Acquire lock
        self.acquire_lock().await?;

        // Validate state before saving
        if self.config.validate_on_load {
            state.validate()?;
        }

        // Serialize state
        let serialized =
            serde_json::to_string_pretty(state).map_err(|e| StateError::SaveFailed {
                reason: format!("Failed to serialize state: {}", e),
            })?;

        // Write to temporary file first (atomic operation)
        let temp_file_path = self.state_file_path.with_extension("tmp");

        {
            let mut file =
                fs::File::create(&temp_file_path).map_err(|e| StateError::SaveFailed {
                    reason: format!("Failed to create temp file: {}", e),
                })?;

            file.write_all(serialized.as_bytes())
                .map_err(|e| StateError::SaveFailed {
                    reason: format!("Failed to write state: {}", e),
                })?;

            file.sync_all().map_err(|e| StateError::SaveFailed {
                reason: format!("Failed to sync file: {}", e),
            })?;
        }

        // Atomic rename
        fs::rename(&temp_file_path, &self.state_file_path).map_err(|e| StateError::SaveFailed {
            reason: format!("Failed to rename temp file: {}", e),
        })?;

        // Get file size
        let file_size_bytes = fs::metadata(&self.state_file_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let save_duration = start_time.elapsed().unwrap_or_default();

        Ok(SaveStateResult {
            success: true,
            file_size_bytes,
            save_duration,
        })
    }

    /// Load release state from file
    pub async fn load_state(&mut self) -> Result<LoadStateResult> {
        // Acquire lock
        self.acquire_lock().await?;

        let warnings = Vec::new();
        let recovered_from_backup = false;

        // Load from main state file
        let state =
            self.load_from_file(&self.state_file_path)
                .map_err(|e| StateError::LoadFailed {
                    reason: format!("Failed to load state file: {}", e),
                })?;

        // Validate loaded state
        if self.config.validate_on_load {
            state.validate()?;
        }

        Ok(LoadStateResult {
            state,
            recovered_from_backup,
            warnings,
        })
    }

    /// Check if state file exists
    pub fn state_exists(&self) -> bool {
        self.state_file_path.exists()
    }

    /// Delete state files
    pub fn cleanup_state(&self) -> Result<()> {
        let mut errors = Vec::new();

        // Remove main state file
        if self.state_file_path.exists()
            && let Err(e) = fs::remove_file(&self.state_file_path)
        {
            errors.push(format!("Failed to remove state file: {}", e));
        }

        // Remove lock file
        if self.lock_file_path.exists()
            && let Err(e) = fs::remove_file(&self.lock_file_path)
        {
            errors.push(format!("Failed to remove lock file: {}", e));
        }

        if !errors.is_empty() {
            return Err(StateError::SaveFailed {
                reason: format!("Cleanup errors: {}", errors.join("; ")),
            }
            .into());
        }

        Ok(())
    }

    /// Get state file information
    pub fn get_state_info(&self) -> Result<StateFileInfo> {
        let main_info = if self.state_file_path.exists() {
            let metadata =
                fs::metadata(&self.state_file_path).map_err(|e| StateError::LoadFailed {
                    reason: format!("Failed to get state file metadata: {}", e),
                })?;

            Some(FileInfo {
                size_bytes: metadata.len(),
                modified_at: metadata.modified().ok(),
                created_at: metadata.created().ok(),
            })
        } else {
            None
        };

        let is_locked = self.lock_file_path.exists();

        Ok(StateFileInfo {
            state_file_path: self.state_file_path.clone(),
            main_file_info: main_info,
            is_locked,
        })
    }

    /// Check if another process has locked the state
    pub fn is_locked_by_other_process(&self) -> bool {
        if !self.lock_file_path.exists() {
            return false;
        }

        // Try to read lock file
        match fs::read_to_string(&self.lock_file_path) {
            Ok(content) => {
                // Try parsing as JSON first (new format)
                if let Ok(lock_info) = serde_json::from_str::<serde_json::Value>(&content)
                    && let Some(pid) = lock_info["pid"].as_u64()
                {
                    return pid as u32 != std::process::id();
                }
                // Fall back to plain PID format (backward compatibility)
                if let Ok(pid) = content.trim().parse::<u32>() {
                    return pid != std::process::id();
                }
                false
            }
            Err(_) => false,
        }
    }

    /// Check if the lock file is stale and can be removed
    ///
    /// A lock is considered stale if:
    /// 1. It's older than stale_lock_timeout_seconds, OR
    /// 2. The process that created it no longer exists (Unix only)
    #[deprecated(note = "Stale detection now handled by flock in acquire_lock()")]
    fn is_lock_stale(&self) -> Result<bool> {
        if !self.lock_file_path.exists() {
            return Ok(false);
        }

        // Read and parse lock file
        let lock_data =
            fs::read_to_string(&self.lock_file_path).map_err(|e| StateError::LoadFailed {
                reason: format!("Failed to read lock file: {}", e),
            })?;

        let lock_info: serde_json::Value =
            serde_json::from_str(&lock_data).map_err(|e| StateError::Corrupted {
                reason: format!("Invalid lock file format: {}", e),
            })?;

        // Check 1: Age-based expiration
        if let Some(acquired_at) = lock_info["acquired_at"].as_u64() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| StateError::LoadFailed {
                    reason: format!("System time error: {}", e),
                })?
                .as_secs();
            let age = now - acquired_at;

            if age > self.config.stale_lock_timeout_seconds {
                log::warn!(
                    "Lock file is stale (age: {}s > {}s)",
                    age,
                    self.config.stale_lock_timeout_seconds
                );
                return Ok(true);
            }
        }

        // Check 2: Process liveness (Unix only)
        #[cfg(unix)]
        if let Some(pid) = lock_info["pid"].as_u64() {
            use std::process::Command;

            // Use `kill -0 <pid>` to check if process exists
            // Exit code 0 = process exists, non-zero = process doesn't exist
            let process_exists = Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if !process_exists {
                log::warn!("Lock file references dead process (PID: {})", pid);
                return Ok(true);
            }
        }

        // On Windows, only age-based detection is used
        #[cfg(not(unix))]
        {
            let _ = lock_info["pid"].as_u64(); // Suppress unused warning
        }

        Ok(false)
    }

    /// Force remove lock (use with caution)
    pub fn force_unlock(&mut self) -> Result<()> {
        if self.lock_file_path.exists() {
            fs::remove_file(&self.lock_file_path).map_err(|e| StateError::SaveFailed {
                reason: format!("Failed to remove lock file: {}", e),
            })?;
        }

        self.lock_handle = None;
        Ok(())
    }

    /// Load state from specific file
    fn load_from_file(&self, file_path: &Path) -> Result<ReleaseState> {
        let mut file = fs::File::open(file_path).map_err(|e| StateError::LoadFailed {
            reason: format!("Failed to open file {}: {}", file_path.display(), e),
        })?;

        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(|e| StateError::LoadFailed {
                reason: format!("Failed to read file {}: {}", file_path.display(), e),
            })?;

        let state: ReleaseState =
            serde_json::from_str(&contents).map_err(|e| StateError::Corrupted {
                reason: format!("Failed to deserialize state: {}", e),
            })?;

        Ok(state)
    }

    /// Acquire file lock using advisory locking (flock)
    async fn acquire_lock(&mut self) -> Result<()> {
        if self.lock_handle.is_some() {
            return Ok(()); // Already locked
        }

        let start_time = SystemTime::now();
        let timeout = Duration::from_millis(self.config.lock_timeout_ms);

        loop {
            // Check timeout
            if start_time.elapsed().unwrap_or_default() >= timeout {
                return Err(StateError::SaveFailed {
                    reason: "Timeout waiting for file lock".to_string(),
                }
                .into());
            }

            // ATOMIC STEP 1: Open or create lock file
            let mut file = match fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)  // Changed from create_new(true) - eliminates TOCTOU race
                .open(&self.lock_file_path)
            {
                Ok(f) => f,
                Err(e) => {
                    return Err(StateError::SaveFailed {
                        reason: format!("Failed to open lock file: {}", e),
                    }
                    .into());
                }
            };

            // ATOMIC STEP 2: Try to acquire flock (this is the REAL lock)
            #[cfg(unix)]
            {
                use nix::fcntl::{flock, FlockArg};
                use std::os::unix::io::AsRawFd;

                match flock(file.as_raw_fd(), FlockArg::LockExclusiveNonblock) {
                    Ok(()) => {
                        // SUCCESS: We hold the exclusive lock
                        // Write our PID to the lock file
                        let pid = std::process::id();
                        let acquired_at = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_err(|e| StateError::SaveFailed {
                                reason: format!("System time error: {}", e),
                            })?
                            .as_secs();

                        let lock_data = serde_json::json!({
                            "pid": pid,
                            "acquired_at": acquired_at,
                        });

                        let lock_data_str = serde_json::to_string(&lock_data)
                            .map_err(|e| StateError::SaveFailed {
                                reason: format!("Failed to serialize lock data: {}", e),
                            })?;

                        // Truncate and write (we hold the lock)
                        file.set_len(0).ok();
                        file.write_all(lock_data_str.as_bytes())
                            .map_err(|e| StateError::SaveFailed {
                                reason: format!("Failed to write lock file: {}", e),
                            })?;
                        file.sync_all().ok();

                        // Store lock handle (keeps flock alive)
                        self.lock_handle = Some(FileLock {
                            _lock_file: self.lock_file_path.clone(),
                            _pid: pid,
                            _acquired_at: SystemTime::now(),
                            _lock_handle: file,  // Must keep alive
                        });

                        return Ok(());
                    }
                    Err(e) if e == nix::errno::Errno::EWOULDBLOCK => {
                        // Lock held by another process - wait and retry
                        drop(file);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    Err(e) => {
                        return Err(StateError::SaveFailed {
                            reason: format!("flock error: {}", e),
                        }
                        .into());
                    }
                }
            }

            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                use windows::Win32::Foundation::HANDLE;
                use windows::Win32::Storage::FileSystem::{
                    LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
                };
                use windows::Win32::System::IO::OVERLAPPED;

                let raw_handle = file.as_raw_handle();
                let handle = HANDLE(raw_handle as isize);
                
                // Zero-initialized OVERLAPPED is correct for synchronous file locking
                let mut overlapped = OVERLAPPED::default();

                // SAFETY:
                // - handle is valid (file was successfully opened)
                // - overlapped is zero-initialized
                // - non-blocking attempt (LOCKFILE_FAIL_IMMEDIATELY)
                unsafe {
                    match LockFileEx(
                        handle,
                        LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                        0,        // Reserved
                        u32::MAX, // Lock entire file
                        u32::MAX,
                        &mut overlapped,
                    ) {
                        Ok(()) => {
                            // SUCCESS: Lock file exists but NOT locked = STALE
                            // We now hold the lock, safe to remove and recreate
                            let pid = std::process::id();
                            let acquired_at = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map_err(|e| StateError::SaveFailed {
                                    reason: format!("System time error: {}", e),
                                })?
                                .as_secs();

                            let lock_data = serde_json::json!({
                                "pid": pid,
                                "acquired_at": acquired_at,
                            });

                            let lock_data_str = serde_json::to_string(&lock_data)
                                .map_err(|e| StateError::SaveFailed {
                                    reason: format!("Failed to serialize lock data: {}", e),
                                })?;

                            file.set_len(0).ok();
                            file.write_all(lock_data_str.as_bytes())
                                .map_err(|e| StateError::SaveFailed {
                                    reason: format!("Failed to write lock file: {}", e),
                                })?;
                            file.sync_all().ok();

                            self.lock_handle = Some(FileLock {
                                _lock_file: self.lock_file_path.clone(),
                                _pid: pid,
                                _acquired_at: SystemTime::now(),
                                _lock_handle: file,
                            });

                            return Ok(());
                        }
                        Err(e) => {
                            // Lock is held by another process or other error
                            let code = e.code().0 as u32;
                            if code == 33 {  // ERROR_LOCK_VIOLATION
                                log::debug!("Lock held by another process, waiting...");
                            }
                            drop(file);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    }
                }
            }

            #[cfg(not(any(unix, windows)))]
            {
                // Fallback: best-effort with age-based detection
                #[allow(deprecated)]
                let is_stale = file.metadata().ok().and_then(|m| m.modified().ok())
                    .and_then(|t| t.elapsed().ok())
                    .map(|d| d.as_secs() > self.config.stale_lock_timeout_seconds)
                    .unwrap_or(false);
                
                if is_stale {
                    // Lock file is stale, overwrite it
                    let pid = std::process::id();
                    let acquired_at = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| StateError::SaveFailed {
                            reason: format!("System time error: {}", e),
                        })?
                        .as_secs();

                    let lock_data = serde_json::json!({
                        "pid": pid,
                        "acquired_at": acquired_at,
                    });

                    let lock_data_str = serde_json::to_string(&lock_data)
                        .map_err(|e| StateError::SaveFailed {
                            reason: format!("Failed to serialize lock data: {}", e),
                        })?;

                    file.set_len(0).ok();
                    file.write_all(lock_data_str.as_bytes())
                        .map_err(|e| StateError::SaveFailed {
                            reason: format!("Failed to write lock file: {}", e),
                        })?;
                    
                    self.lock_handle = Some(FileLock {
                        _lock_file: self.lock_file_path.clone(),
                        _pid: pid,
                        _acquired_at: SystemTime::now(),
                        _lock_handle: file,
                    });
                    
                    return Ok(());
                } else {
                    drop(file);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Update configuration
    pub fn set_config(&mut self, config: StateConfig) {
        self.config = config;
    }

    /// Get configuration
    pub fn config(&self) -> &StateConfig {
        &self.config
    }
}

impl Drop for StateManager {
    fn drop(&mut self) {
        // Release lock when manager is dropped
        if self.lock_handle.is_some() {
            let _ = fs::remove_file(&self.lock_file_path);
        }
    }
}

/// Information about state files
#[derive(Debug, Clone)]
pub struct StateFileInfo {
    /// Path to main state file
    pub state_file_path: PathBuf,
    /// Information about main state file
    pub main_file_info: Option<FileInfo>,
    /// Whether state is currently locked
    pub is_locked: bool,
}

/// Information about a single file
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// File size in bytes
    pub size_bytes: u64,
    /// Last modified time
    pub modified_at: Option<SystemTime>,
    /// Created time
    pub created_at: Option<SystemTime>,
}

impl StateFileInfo {
    /// Check if state files exist
    pub fn has_state(&self) -> bool {
        self.main_file_info.is_some()
    }

    /// Get total size of all state files
    pub fn total_size_bytes(&self) -> u64 {
        self.main_file_info
            .as_ref()
            .map(|f| f.size_bytes)
            .unwrap_or(0)
    }

    /// Format state info for display
    pub fn format_info(&self) -> String {
        let mut info = String::new();

        if let Some(main_info) = &self.main_file_info {
            info.push_str(&format!("State: {} bytes", main_info.size_bytes));
            if let Some(modified) = main_info.modified_at
                && let Ok(elapsed) = modified.elapsed()
            {
                info.push_str(&format!(" (modified {}s ago)", elapsed.as_secs()));
            }
        } else {
            info.push_str("No state file");
        }

        if self.is_locked {
            info.push_str(" [LOCKED]");
        }

        info
    }
}

impl SaveStateResult {
    /// Format save result for display
    pub fn format_result(&self) -> String {
        if self.success {
            format!(
                "✅ State saved: {} bytes in {:.2}s",
                self.file_size_bytes,
                self.save_duration.as_secs_f64()
            )
        } else {
            "❌ Failed to save state".to_string()
        }
    }
}

impl LoadStateResult {
    /// Format load result for display
    pub fn format_result(&self) -> String {
        let mut result = if self.recovered_from_backup {
            "⚠️ State loaded from backup".to_string()
        } else {
            "✅ State loaded successfully".to_string()
        };

        if !self.warnings.is_empty() {
            result.push_str(&format!(" ({} warnings)", self.warnings.len()));
        }

        result
    }
}

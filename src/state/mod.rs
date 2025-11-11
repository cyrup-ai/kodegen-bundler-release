//! State management for release operations.
//!
//! This module provides state tracking and persistence for release operations,
//! enabling resume capabilities and ensuring atomic operations.

mod manager;
mod release_state;

pub use manager::{
    LoadStateResult, SaveStateResult, StateManager,
};
pub use release_state::{
    ReleaseConfig, ReleasePhase, ReleaseState,
};

use crate::error::Result;

/// Create a state manager for the state file in the given temp directory
pub fn create_state_manager(temp_dir: &std::path::Path) -> Result<StateManager> {
    let state_file = temp_dir.join(".cyrup_release_state.json");
    StateManager::new(state_file)
}

/// Quick check if release state exists in the given temp directory
pub fn has_active_release(temp_dir: &std::path::Path) -> bool {
    temp_dir.join(".cyrup_release_state.json").exists()
}

/// Load release state from the given temp directory
pub async fn load_release_state(temp_dir: &std::path::Path) -> Result<LoadStateResult> {
    let mut manager = create_state_manager(temp_dir)?;
    manager.load_state().await
}

/// Save release state to the given temp directory
pub async fn save_release_state(
    temp_dir: &std::path::Path,
    state: &mut ReleaseState
) -> Result<SaveStateResult> {
    let mut manager = create_state_manager(temp_dir)?;
    manager.save_state(state).await
}

/// Cleanup release state in the given temp directory
pub fn cleanup_release_state(temp_dir: &std::path::Path) -> Result<()> {
    let manager = create_state_manager(temp_dir)?;
    manager.cleanup_state()
}

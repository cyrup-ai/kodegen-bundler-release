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

/// Create a state manager for the default state file location
pub fn create_state_manager() -> Result<StateManager> {
    StateManager::new(".cyrup_release_state.json")
}

/// Quick check if release state exists at default location
pub fn has_active_release() -> bool {
    std::path::Path::new(".cyrup_release_state.json").exists()
}

/// Load release state from default location
pub async fn load_release_state() -> Result<LoadStateResult> {
    let mut manager = create_state_manager()?;
    manager.load_state().await
}

/// Save release state to default location
pub async fn save_release_state(state: &ReleaseState) -> Result<SaveStateResult> {
    let mut manager = create_state_manager()?;
    manager.save_state(state).await
}

/// Cleanup release state at default location
pub fn cleanup_release_state() -> Result<()> {
    let manager = create_state_manager()?;
    manager.cleanup_state()
}

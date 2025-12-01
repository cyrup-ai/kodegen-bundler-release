//! State management for release operations.

mod manager;
mod release_state;

pub use manager::{SaveStateResult, StateManager};
pub use release_state::{ReleaseConfig, ReleasePhase, ReleaseState};

use crate::error::Result;

fn create_state_manager(temp_dir: &std::path::Path) -> Result<StateManager> {
    let state_file = temp_dir.join(".cyrup_release_state.json");
    StateManager::new(state_file)
}

/// Save release state to the given temp directory
pub async fn save_release_state(
    temp_dir: &std::path::Path,
    state: &mut ReleaseState,
) -> Result<SaveStateResult> {
    let mut manager = create_state_manager(temp_dir)?;
    manager.save_state(state).await
}

/// Cleanup release state in the given temp directory
pub fn cleanup_release_state(temp_dir: &std::path::Path) -> Result<()> {
    let manager = create_state_manager(temp_dir)?;
    manager.cleanup_state()
}

//! Command execution functions coordinating all release operations.
//!
//! This module implements the complete release workflow by coordinating
//! all modules and providing comprehensive error handling and user feedback.

// Submodules
mod helpers;
mod release;
mod temp_clone;

use crate::cli::{Args, RuntimeConfig};
use crate::error::Result;

// Import command executors
use release::execute_release;

/// Execute the main command based on parsed arguments
pub async fn execute_command(args: Args) -> Result<i32> {
    // Validate arguments
    if let Err(validation_error) = args.validate() {
        let output = super::OutputManager::new(false, false);
        output.error(&format!("Invalid arguments: {}", validation_error));
        return Ok(1);
    }

    let config = RuntimeConfig::from(&args);

    // Execute release command
    let result = execute_release(&args, &config).await;

    match result {
        Ok(exit_code) => {
            // Don't print success message here - release command already did
            Ok(exit_code)
        }
        Err(e) => {
            config.error_println(&format!("Release failed: {}", e));

            // Show recovery suggestions if available
            if config.is_verbose() {
                let suggestions = e.recovery_suggestions();
                if !suggestions.is_empty() {
                    config.println("\n💡 Recovery suggestions:");
                    for suggestion in suggestions {
                        config.println(&format!("  • {}", suggestion));
                    }
                }
            }

            Ok(1)
        }
    }
}

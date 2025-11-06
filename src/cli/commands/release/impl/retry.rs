//! Retry logic with exponential backoff for network operations.

use crate::cli::RuntimeConfig;
use crate::error::{PublishError, ReleaseError, Result};

/// Maximum backoff time in seconds (1 hour)
/// Prevents exponential backoff from producing impractical wait times
const MAX_BACKOFF_SECONDS: u64 = 3600;

/// Retry an async operation with exponential backoff
///
/// This helper automatically retries recoverable errors with intelligent backoff:
/// - Network/transient errors: Exponential backoff (1s, 2s, 4s, 8s)
/// - Rate limit errors: Wait exact time specified in error
/// - Unrecoverable errors: Return immediately without retry
///
/// # Arguments
/// * `operation` - Async closure that returns Result<T>
/// * `max_retries` - Maximum number of retry attempts (0 = try once, no retries)
/// * `operation_name` - Human-readable name for logging
/// * `config` - Runtime config for user messaging
///
/// # Returns
/// * `Ok(T)` - Operation succeeded (possibly after retries)
/// * `Err(ReleaseError)` - Operation failed after all retries, or unrecoverable error
pub async fn retry_with_backoff<F, T, Fut>(
    mut operation: F,
    max_retries: u32,
    operation_name: &str,
    config: &RuntimeConfig,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempts = 0;
    
    loop {
        match operation().await {
            Ok(result) => {
                if attempts > 0 {
                    config.success_println(&format!(
                        "✓ {} succeeded after {} retry(ies)",
                        operation_name,
                        attempts
                    ));
                }
                return Ok(result);
            }
            Err(e) => {
                // Check if error is recoverable
                if !e.is_recoverable() {
                    // Unrecoverable error - fail immediately, no retries
                    config.error_println(&format!(
                        "❌ {} failed with unrecoverable error",
                        operation_name
                    ));
                    return Err(e);
                }
                
                // Recoverable error - check if we have retries left
                if attempts >= max_retries {
                    // Retries exhausted
                    config.error_println(&format!(
                        "❌ {} failed after {} attempt(s)",
                        operation_name,
                        attempts + 1
                    ));
                    return Err(e);
                }
                
                attempts += 1;
                
                // Determine wait time based on error type
                let wait_seconds = match &e {
                    ReleaseError::Publish(PublishError::RateLimitExceeded { retry_after_seconds }) => {
                        // Use the exact wait time from the error (but still cap it)
                        (*retry_after_seconds).min(MAX_BACKOFF_SECONDS)
                    }
                    _ => {
                        // Exponential backoff with overflow protection: 1s, 2s, 4s, 8s, ..., max 3600s
                        // Use saturating_pow to prevent panic, then cap at maximum
                        2u64.saturating_pow(attempts - 1).min(MAX_BACKOFF_SECONDS)
                    }
                };
                
                // Log retry attempt
                config.warning_println(&format!(
                    "⚠️  {} failed (attempt {}/{}): {}",
                    operation_name,
                    attempts,
                    max_retries + 1,
                    e
                ));
                config.indent(&format!("   Retrying in {}s...", wait_seconds));
                
                // Wait before retry
                tokio::time::sleep(tokio::time::Duration::from_secs(wait_seconds)).await;
            }
        }
    }
}

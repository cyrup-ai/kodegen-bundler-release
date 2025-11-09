//! Retry logic with exponential backoff for network operations.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, PublishError, ReleaseError, Result};
use tokio::time::{Duration, Instant};

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
/// * `absolute_timeout` - Optional absolute timeout for the entire retry operation (default: 30 minutes)
///
/// # Returns
/// * `Ok(T)` - Operation succeeded (possibly after retries)
/// * `Err(ReleaseError)` - Operation failed after all retries, unrecoverable error, or timeout
pub async fn retry_with_backoff<F, T, Fut>(
    mut operation: F,
    max_retries: u32,
    operation_name: &str,
    config: &RuntimeConfig,
    absolute_timeout: Option<Duration>,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    // Default absolute timeout: 30 minutes
    let start_time = Instant::now();
    let deadline = start_time + absolute_timeout.unwrap_or(Duration::from_secs(1800));
    
    let mut attempts = 0;
    
    loop {
        // Check absolute timeout before attempting operation
        if Instant::now() >= deadline {
            let total_time = Instant::now().duration_since(start_time);
            return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                command: operation_name.to_string(),
                reason: format!(
                    "Operation timed out after {} attempts over {:.1}s",
                    attempts,
                    total_time.as_secs_f64()
                ),
            }));
        }
        
        match operation().await {
            Ok(result) => {
                if attempts > 0 {
                    config.success_println(&format!(
                        "✓ {} succeeded after {} retry(ies)",
                        operation_name,
                        attempts
                    )).expect("Failed to write to stdout");
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
                
                // Calculate remaining time until deadline
                let remaining_time = deadline.saturating_duration_since(Instant::now());
                let actual_wait = Duration::from_secs(wait_seconds).min(remaining_time);
                
                // Check if we have any time left
                if actual_wait.is_zero() {
                    return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                        command: operation_name.to_string(),
                        reason: "Absolute timeout reached before retry could be attempted".to_string(),
                    }));
                }
                
                // Log retry attempt
                config.warning_println(&format!(
                    "⚠️  {} failed (attempt {}/{}): {}",
                    operation_name,
                    attempts,
                    max_retries + 1,
                    e
                )).expect("Failed to write to stdout");
                config.indent(&format!("   Retrying in {:.1}s...", actual_wait.as_secs_f64())).expect("Failed to write to stdout");
                
                // Wait before retry (respecting deadline)
                tokio::time::sleep(actual_wait).await;
            }
        }
    }
}

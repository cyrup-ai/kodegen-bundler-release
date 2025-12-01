//! Retry logic with exponential backoff for network operations.

use crate::cli::RuntimeConfig;
use crate::error::{CliError, ReleaseError, Result};
use tokio::time::{Duration, Instant};

/// Maximum backoff time in seconds (1 hour)
const MAX_BACKOFF_SECONDS: u64 = 3600;

/// Retry an async operation with exponential backoff
///
/// This helper automatically retries recoverable errors with intelligent backoff:
/// - Network/transient errors: Exponential backoff (1s, 2s, 4s, 8s)
/// - Unrecoverable errors: Return immediately without retry
///
/// # Arguments
/// * `operation` - Async closure that returns Result<T>
/// * `max_retries` - Maximum number of retry attempts (0 = try once, no retries)
/// * `operation_name` - Human-readable name for logging
/// * `config` - Runtime config for user messaging
/// * `absolute_timeout` - Optional absolute timeout for the entire retry operation (default: 30 minutes)
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
    let start_time = Instant::now();
    let deadline = start_time + absolute_timeout.unwrap_or(Duration::from_secs(1800));

    let mut attempts = 0;

    loop {
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
                    config
                        .success_println(&format!(
                            "✓ {} succeeded after {} retry(ies)",
                            operation_name, attempts
                        ))
                        .expect("Failed to write to stdout");
                }
                return Ok(result);
            }
            Err(e) => {
                if !e.is_recoverable() {
                    config.error_println(&format!(
                        "❌ {} failed with unrecoverable error",
                        operation_name
                    ));
                    return Err(e);
                }

                if attempts >= max_retries {
                    config.error_println(&format!(
                        "❌ {} failed after {} attempt(s)",
                        operation_name,
                        attempts + 1
                    ));
                    return Err(e);
                }

                attempts += 1;

                // Exponential backoff: 1s, 2s, 4s, 8s, ..., max 3600s
                let wait_seconds = 2u64.saturating_pow(attempts - 1).min(MAX_BACKOFF_SECONDS);

                let remaining_time = deadline.saturating_duration_since(Instant::now());
                let actual_wait = Duration::from_secs(wait_seconds).min(remaining_time);

                if actual_wait.is_zero() {
                    return Err(ReleaseError::Cli(CliError::ExecutionFailed {
                        command: operation_name.to_string(),
                        reason: "Absolute timeout reached before retry could be attempted"
                            .to_string(),
                    }));
                }

                config
                    .warning_println(&format!(
                        "⚠️  {} failed (attempt {}/{}): {}",
                        operation_name,
                        attempts,
                        max_retries + 1,
                        e
                    ))
                    .expect("Failed to write to stdout");
                config
                    .indent(&format!(
                        "   Retrying in {:.1}s...",
                        actual_wait.as_secs_f64()
                    ))
                    .expect("Failed to write to stdout");

                tokio::time::sleep(actual_wait).await;
            }
        }
    }
}

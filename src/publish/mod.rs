//! Publishing operations for crates.io
//!
//! Provides CargoPublisher for reliable cargo publish operations with:
//! - Exponential backoff retry logic
//! - Rate limit detection and handling
//! - Structured results with warnings and duration tracking
//! - Additional operations: yank, version checking, dry-run validation

mod cargo_ops;

pub use cargo_ops::PublishResult;

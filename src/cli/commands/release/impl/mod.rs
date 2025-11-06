//! Release implementation modules.
//!
//! This module contains the decomposed release implementation logic:
//! - `context`: Context structure for phase execution
//! - `retry`: Retry logic with exponential backoff
//! - `platform`: Platform detection and bundling operations
//! - `phases`: Release phase execution (phases 2-8)
//! - `release`: Main release orchestration logic

mod context;
mod retry;
mod platform;
mod phases;
mod release;

// Re-export the main entry point
pub use release::perform_release_single_repo;

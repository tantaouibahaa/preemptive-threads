//! Thread scheduler implementations.
//!
//! Provides the round-robin scheduler for managing thread execution.

pub mod rr;
pub mod trait_def;

pub use rr::RoundRobinScheduler;
pub use trait_def::{priority, CpuId, Scheduler};

/// Default scheduler type.
pub type DefaultScheduler = RoundRobinScheduler;

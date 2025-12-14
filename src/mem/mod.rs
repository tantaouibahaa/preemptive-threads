//! Memory management for thread stacks.
//!
//! Provides safe abstractions for managing thread stacks and
//! reference counting in a no_std environment.

pub mod arc_lite;
pub mod stack_pool;

pub use arc_lite::ArcLite;
pub use stack_pool::{Stack, StackPool, StackSizeClass};

//! Memory management: the kernel heap and the thread stack pool.

pub mod heap;
pub mod stack;

pub use heap::{heap_stats, HeapStats};
pub use stack::{Stack, StackPool, StackPoolStats, DEFAULT_STACK_SIZE, MIN_STACK_SIZE};

//! Memory management: the kernel heap, the thread stack pool and the
//! physical frame allocator.

pub mod frame;
pub mod heap;
pub mod stack;

pub use frame::{frame_stats, FrameStats};
pub use heap::{heap_stats, HeapStats};
pub use stack::{Stack, StackPool, StackPoolStats, DEFAULT_STACK_SIZE, MIN_STACK_SIZE};

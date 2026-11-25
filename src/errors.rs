//! Error types.

use core::fmt;

/// Why a thread could not be spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// No memory for the stack or control block.
    OutOfMemory,
    /// The requested CPU affinity does not name a core.
    InvalidCpu,
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::OutOfMemory => write!(f, "out of memory"),
            SpawnError::InvalidCpu => write!(f, "invalid CPU affinity"),
        }
    }
}

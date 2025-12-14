//! Comprehensive error handling for the threading system.
//!
//! This module provides detailed error types for all threading operations,
//! enabling proper error handling and debugging throughout the system.

#![allow(clippy::uninlined_format_args)]

use core::fmt;
extern crate alloc;
use alloc::string::String;

/// Result type for threading operations.
pub type ThreadResult<T> = Result<T, ThreadError>;

/// Comprehensive error type for all threading operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadError {
    /// Thread spawning errors
    Spawn(SpawnError),
    /// Thread joining errors  
    Join(JoinError),
    /// Scheduling errors
    Schedule(ScheduleError),
    /// Memory allocation errors
    Memory(MemoryError),
    /// Timer and timing errors
    Timer(TimerError),
    /// Architecture-specific errors
    Arch(ArchError),
    /// Thread-local storage errors
    Tls(TlsError),
    /// Permission and security errors
    Permission(PermissionError),
    /// Resource limit errors
    Resource(ResourceError),
    /// Invalid operation errors
    InvalidOperation(InvalidOperationError),
}

/// Errors that can occur during thread spawning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// System is not initialized
    NotInitialized,
    /// Out of memory for stack allocation
    OutOfMemory,
    /// Maximum number of threads reached
    TooManyThreads,
    /// Invalid stack size specified
    InvalidStackSize(usize),
    /// Invalid priority specified
    InvalidPriority(u8),
    /// Invalid CPU affinity specified
    InvalidAffinity(u64),
    /// Thread name is invalid or too long
    InvalidName(String),
    /// Architecture does not support requested feature
    UnsupportedFeature(String),
    /// Scheduler rejected the thread
    SchedulerRejected,
}

/// Errors that can occur during thread joining.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    /// Thread has already been joined
    AlreadyJoined,
    /// Thread panicked during execution
    ThreadPanicked,
    /// Thread was terminated abnormally
    Terminated,
    /// Join operation timed out
    Timeout,
    /// Thread is still running (for try_join)
    StillRunning,
    /// Invalid thread handle
    InvalidHandle,
}

/// Errors related to scheduling operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// No schedulable threads available
    NoThreadsAvailable,
    /// Scheduler is in an invalid state
    InvalidState,
    /// CPU does not exist or is offline
    InvalidCpu(usize),
    /// Priority change not allowed
    PriorityChangeNotAllowed,
    /// Scheduler queue is full
    QueueFull,
    /// Preemption is disabled
    PreemptionDisabled,
}

/// Memory-related errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// Out of memory
    OutOfMemory,
    /// Stack overflow detected
    StackOverflow,
    /// Stack underflow detected
    StackUnderflow,
    /// Invalid memory address
    InvalidAddress(usize),
    /// Memory alignment error
    AlignmentError,
    /// Memory pool exhausted
    PoolExhausted,
    /// Invalid memory layout
    InvalidLayout,
}

/// Timer and timing related errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerError {
    /// Timer not initialized
    NotInitialized,
    /// Timer already running
    AlreadyRunning,
    /// Timer not running
    NotRunning,
    /// Invalid timer frequency
    InvalidFrequency(u32),
    /// Timer hardware not available
    HardwareNotAvailable,
    /// Invalid timer configuration
    InvalidConfig,
}

/// Architecture-specific errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchError {
    /// Unsupported architecture
    UnsupportedArchitecture,
    /// Context switch failed
    ContextSwitchFailed,
    /// Invalid CPU state
    InvalidCpuState,
    /// Interrupt handling error
    InterruptError,
    /// FPU operation error
    FpuError,
    /// Invalid instruction
    InvalidInstruction,
}

/// Thread-local storage errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsError {
    /// TLS key not found
    KeyNotFound,
    /// TLS storage exhausted
    StorageExhausted,
    /// Invalid TLS key
    InvalidKey,
    /// TLS data corrupted
    DataCorrupted,
    /// TLS not supported
    NotSupported,
}

/// Permission and security errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionError {
    /// Operation not permitted
    NotPermitted,
    /// Access denied
    AccessDenied,
    /// Insufficient privileges
    InsufficientPrivileges,
    /// Security policy violation
    SecurityViolation,
    /// Operation would compromise security
    SecurityRisk,
}

/// Resource limit errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    /// Maximum threads per process exceeded
    MaxThreadsPerProcess,
    /// Maximum threads per user exceeded
    MaxThreadsPerUser,
    /// Maximum memory usage exceeded
    MaxMemoryUsage,
    /// Maximum CPU time exceeded
    MaxCpuTime,
    /// Maximum file descriptors exceeded
    MaxFileDescriptors,
    /// Resource temporarily unavailable
    ResourceUnavailable,
}

/// Invalid operation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidOperationError {
    /// Operation called on wrong thread
    WrongThread,
    /// Operation called in wrong state
    WrongState,
    /// Invalid parameter provided
    InvalidParameter(String),
    /// Operation not supported in current context
    NotSupported,
    /// Deadlock would occur
    WouldDeadlock,
    /// Operation already in progress
    AlreadyInProgress,
}

// Display implementations for user-friendly error messages

impl fmt::Display for ThreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThreadError::Spawn(e) => write!(f, "Thread spawn error: {}", e),
            ThreadError::Join(e) => write!(f, "Thread join error: {}", e),
            ThreadError::Schedule(e) => write!(f, "Scheduling error: {}", e),
            ThreadError::Memory(e) => write!(f, "Memory error: {}", e),
            ThreadError::Timer(e) => write!(f, "Timer error: {}", e),
            ThreadError::Arch(e) => write!(f, "Architecture error: {}", e),
            ThreadError::Tls(e) => write!(f, "Thread-local storage error: {}", e),
            ThreadError::Permission(e) => write!(f, "Permission error: {}", e),
            ThreadError::Resource(e) => write!(f, "Resource error: {}", e),
            ThreadError::InvalidOperation(e) => write!(f, "Invalid operation: {}", e),
        }
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::NotInitialized => write!(f, "Threading system not initialized"),
            SpawnError::OutOfMemory => write!(f, "Out of memory for thread creation"),
            SpawnError::TooManyThreads => write!(f, "Maximum number of threads reached"),
            SpawnError::InvalidStackSize(size) => write!(f, "Invalid stack size: {}", size),
            SpawnError::InvalidPriority(prio) => write!(f, "Invalid priority: {}", prio),
            SpawnError::InvalidAffinity(affinity) => write!(f, "Invalid CPU affinity: {:#x}", affinity),
            SpawnError::InvalidName(name) => write!(f, "Invalid thread name: {}", name),
            SpawnError::UnsupportedFeature(feature) => write!(f, "Unsupported feature: {}", feature),
            SpawnError::SchedulerRejected => write!(f, "Scheduler rejected thread creation"),
        }
    }
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JoinError::AlreadyJoined => write!(f, "Thread has already been joined"),
            JoinError::ThreadPanicked => write!(f, "Thread panicked during execution"),
            JoinError::Terminated => write!(f, "Thread was terminated abnormally"),
            JoinError::Timeout => write!(f, "Join operation timed out"),
            JoinError::StillRunning => write!(f, "Thread is still running"),
            JoinError::InvalidHandle => write!(f, "Invalid thread handle"),
        }
    }
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScheduleError::NoThreadsAvailable => write!(f, "No schedulable threads available"),
            ScheduleError::InvalidState => write!(f, "Scheduler is in an invalid state"),
            ScheduleError::InvalidCpu(cpu) => write!(f, "Invalid CPU ID: {}", cpu),
            ScheduleError::PriorityChangeNotAllowed => write!(f, "Priority change not allowed"),
            ScheduleError::QueueFull => write!(f, "Scheduler queue is full"),
            ScheduleError::PreemptionDisabled => write!(f, "Preemption is disabled"),
        }
    }
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::OutOfMemory => write!(f, "Out of memory"),
            MemoryError::StackOverflow => write!(f, "Stack overflow detected"),
            MemoryError::StackUnderflow => write!(f, "Stack underflow detected"),
            MemoryError::InvalidAddress(addr) => write!(f, "Invalid memory address: {:#x}", addr),
            MemoryError::AlignmentError => write!(f, "Memory alignment error"),
            MemoryError::PoolExhausted => write!(f, "Memory pool exhausted"),
            MemoryError::InvalidLayout => write!(f, "Invalid memory layout"),
        }
    }
}

impl fmt::Display for TimerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimerError::NotInitialized => write!(f, "Timer not initialized"),
            TimerError::AlreadyRunning => write!(f, "Timer already running"),
            TimerError::NotRunning => write!(f, "Timer not running"),
            TimerError::InvalidFrequency(freq) => write!(f, "Invalid timer frequency: {} Hz", freq),
            TimerError::HardwareNotAvailable => write!(f, "Timer hardware not available"),
            TimerError::InvalidConfig => write!(f, "Invalid timer configuration"),
        }
    }
}

impl fmt::Display for ArchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArchError::UnsupportedArchitecture => write!(f, "Unsupported architecture"),
            ArchError::ContextSwitchFailed => write!(f, "Context switch failed"),
            ArchError::InvalidCpuState => write!(f, "Invalid CPU state"),
            ArchError::InterruptError => write!(f, "Interrupt handling error"),
            ArchError::FpuError => write!(f, "FPU operation error"),
            ArchError::InvalidInstruction => write!(f, "Invalid instruction"),
        }
    }
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::KeyNotFound => write!(f, "TLS key not found"),
            TlsError::StorageExhausted => write!(f, "TLS storage exhausted"),
            TlsError::InvalidKey => write!(f, "Invalid TLS key"),
            TlsError::DataCorrupted => write!(f, "TLS data corrupted"),
            TlsError::NotSupported => write!(f, "TLS not supported"),
        }
    }
}

impl fmt::Display for PermissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PermissionError::NotPermitted => write!(f, "Operation not permitted"),
            PermissionError::AccessDenied => write!(f, "Access denied"),
            PermissionError::InsufficientPrivileges => write!(f, "Insufficient privileges"),
            PermissionError::SecurityViolation => write!(f, "Security policy violation"),
            PermissionError::SecurityRisk => write!(f, "Operation would compromise security"),
        }
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceError::MaxThreadsPerProcess => write!(f, "Maximum threads per process exceeded"),
            ResourceError::MaxThreadsPerUser => write!(f, "Maximum threads per user exceeded"),
            ResourceError::MaxMemoryUsage => write!(f, "Maximum memory usage exceeded"),
            ResourceError::MaxCpuTime => write!(f, "Maximum CPU time exceeded"),
            ResourceError::MaxFileDescriptors => write!(f, "Maximum file descriptors exceeded"),
            ResourceError::ResourceUnavailable => write!(f, "Resource temporarily unavailable"),
        }
    }
}

impl fmt::Display for InvalidOperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidOperationError::WrongThread => write!(f, "Operation called on wrong thread"),
            InvalidOperationError::WrongState => write!(f, "Operation called in wrong state"),
            InvalidOperationError::InvalidParameter(param) => write!(f, "Invalid parameter: {}", param),
            InvalidOperationError::NotSupported => write!(f, "Operation not supported in current context"),
            InvalidOperationError::WouldDeadlock => write!(f, "Operation would cause deadlock"),
            InvalidOperationError::AlreadyInProgress => write!(f, "Operation already in progress"),
        }
    }
}

// Conversion implementations for ergonomic error handling

impl From<SpawnError> for ThreadError {
    fn from(error: SpawnError) -> Self {
        ThreadError::Spawn(error)
    }
}

impl From<JoinError> for ThreadError {
    fn from(error: JoinError) -> Self {
        ThreadError::Join(error)
    }
}

impl From<ScheduleError> for ThreadError {
    fn from(error: ScheduleError) -> Self {
        ThreadError::Schedule(error)
    }
}

impl From<MemoryError> for ThreadError {
    fn from(error: MemoryError) -> Self {
        ThreadError::Memory(error)
    }
}

impl From<TimerError> for ThreadError {
    fn from(error: TimerError) -> Self {
        ThreadError::Timer(error)
    }
}

impl From<ArchError> for ThreadError {
    fn from(error: ArchError) -> Self {
        ThreadError::Arch(error)
    }
}

impl From<TlsError> for ThreadError {
    fn from(error: TlsError) -> Self {
        ThreadError::Tls(error)
    }
}

impl From<PermissionError> for ThreadError {
    fn from(error: PermissionError) -> Self {
        ThreadError::Permission(error)
    }
}

impl From<ResourceError> for ThreadError {
    fn from(error: ResourceError) -> Self {
        ThreadError::Resource(error)
    }
}

impl From<InvalidOperationError> for ThreadError {
    fn from(error: InvalidOperationError) -> Self {
        ThreadError::InvalidOperation(error)
    }
}

// Convert from old SpawnError to new system
impl From<crate::kernel::SpawnError> for SpawnError {
    fn from(error: crate::kernel::SpawnError) -> Self {
        match error {
            crate::kernel::SpawnError::NotInitialized => SpawnError::NotInitialized,
            crate::kernel::SpawnError::OutOfMemory => SpawnError::OutOfMemory,
            crate::kernel::SpawnError::TooManyThreads => SpawnError::TooManyThreads,
            crate::kernel::SpawnError::InvalidStackSize => SpawnError::InvalidStackSize(0),
        }
    }
}

impl From<crate::time::TimerError> for TimerError {
    fn from(error: crate::time::TimerError) -> Self {
        match error {
            crate::time::TimerError::NotInitialized => TimerError::NotInitialized,
            crate::time::TimerError::AlreadyRunning => TimerError::AlreadyRunning,
            crate::time::TimerError::NotRunning => TimerError::NotRunning,
            crate::time::TimerError::UnsupportedFrequency => TimerError::InvalidFrequency(0),
            crate::time::TimerError::InvalidConfig => TimerError::InvalidConfig,
            crate::time::TimerError::NotAvailable => TimerError::HardwareNotAvailable,
        }
    }
}

// Convenience constructors for common error patterns
impl ThreadError {
    /// Create a memory error.
    pub fn memory_error() -> Self {
        ThreadError::Memory(MemoryError::OutOfMemory)
    }

    /// Create a resource exhaustion error.
    pub fn resource_exhaustion() -> Self {
        ThreadError::Resource(ResourceError::ResourceUnavailable)
    }

    /// Create an invalid state error.
    pub fn invalid_state() -> Self {
        ThreadError::Schedule(ScheduleError::InvalidState)
    }

    /// Create a permission denied error.
    pub fn permission_denied() -> Self {
        ThreadError::Permission(PermissionError::AccessDenied)
    }

    /// Create an unsupported operation error.
    pub fn unsupported_operation(msg: String) -> Self {
        ThreadError::InvalidOperation(InvalidOperationError::InvalidParameter(msg))
    }

    /// Create a generic error with a message.
    pub fn other(msg: String) -> Self {
        ThreadError::InvalidOperation(InvalidOperationError::InvalidParameter(msg))
    }
}
//! Architecture detection and runtime optimization.
//!
//! This module provides runtime detection of CPU features and capabilities
//! for the ARM Cortex-A53 (AArch64) on Raspberry Pi Zero 2 W.

use portable_atomic::{AtomicBool, Ordering};

/// CPU architecture types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuArch {
    Aarch64,
    Unknown,
}

/// CPU feature flags for ARM64 architecture.
#[derive(Debug, Clone, Copy)]
pub struct CpuFeatures {
    pub arch: CpuArch,
    pub cache_line_size: u32,
    pub cpu_cores: u32,
    pub supports_fpu: bool,
    pub supports_vector: bool,
    pub supports_atomic_cas: bool,
    pub supports_memory_ordering: bool,
    pub supports_neon: bool,
    pub supports_sve: bool,
    pub supports_sve2: bool,
}

impl Default for CpuFeatures {
    fn default() -> Self {
        Self {
            arch: CpuArch::Aarch64,
            cache_line_size: 64,
            cpu_cores: 4,
            supports_fpu: true,
            supports_vector: true,
            supports_atomic_cas: true,
            supports_memory_ordering: true,
            supports_neon: true,
            supports_sve: false,
            supports_sve2: false,
        }
    }
}

static CPU_FEATURES: spin::Mutex<Option<CpuFeatures>> = spin::Mutex::new(None);
static DETECTION_DONE: AtomicBool = AtomicBool::new(false);

/// Detect current CPU architecture and features.
pub fn detect_cpu_features() -> CpuFeatures {
    // Fast path - check if already detected
    if DETECTION_DONE.load(Ordering::Acquire) {
        let guard = CPU_FEATURES.lock();
        if let Some(features) = *guard {
            return features;
        }
    }

    // Slow path - perform detection
    let features = perform_detection();

    // Store results
    {
        let mut guard = CPU_FEATURES.lock();
        *guard = Some(features);
    }
    DETECTION_DONE.store(true, Ordering::Release);

    features
}

/// Get cached CPU features (must call detect_cpu_features first).
pub fn get_cpu_features() -> Option<CpuFeatures> {
    if DETECTION_DONE.load(Ordering::Acquire) {
        let guard = CPU_FEATURES.lock();
        *guard
    } else {
        None
    }
}

/// Internal CPU feature detection for ARM Cortex-A53.
fn perform_detection() -> CpuFeatures {
    CpuFeatures {
        arch: CpuArch::Aarch64,
        cache_line_size: 64, // Cortex-A53 has 64-byte cache lines
        cpu_cores: 4,        // RPi Zero 2 W has 4 cores
        supports_fpu: true,  // ARM64 always has FPU
        supports_vector: true,
        supports_atomic_cas: true,
        supports_memory_ordering: true,
        supports_neon: true, // ARM64 always has NEON
        supports_sve: false, // Cortex-A53 doesn't have SVE
        supports_sve2: false,
    }
}

/// Runtime optimization controller.
pub struct RuntimeOptimizer {
    features: CpuFeatures,
}

impl RuntimeOptimizer {
    /// Create a new runtime optimizer with detected CPU features.
    pub fn new() -> Self {
        Self {
            features: detect_cpu_features(),
        }
    }

    /// Get the detected CPU features.
    pub fn features(&self) -> &CpuFeatures {
        &self.features
    }

    /// Choose optimal memory barrier implementation.
    pub fn optimal_memory_barrier(&self) -> fn() {
        || {
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("dmb sy", options(nostack, preserves_flags));
            }

            #[cfg(not(target_arch = "aarch64"))]
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Get optimal cache line size for alignment.
    pub fn optimal_cache_line_size(&self) -> usize {
        self.features.cache_line_size as usize
    }

    /// Determine if lock-free algorithms should be preferred.
    pub fn prefer_lock_free(&self) -> bool {
        self.features.supports_atomic_cas && self.features.supports_memory_ordering
    }

    /// Get recommended number of worker threads.
    pub fn recommended_worker_threads(&self) -> usize {
        (self.features.cpu_cores as usize).max(1)
    }
}

impl Default for RuntimeOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Global runtime optimizer instance.
static GLOBAL_OPTIMIZER: spin::Mutex<Option<RuntimeOptimizer>> = spin::Mutex::new(None);

/// Get the global runtime optimizer instance.
pub fn global_optimizer() -> RuntimeOptimizer {
    let mut guard = GLOBAL_OPTIMIZER.lock();
    if let Some(optimizer) = guard.as_ref() {
        RuntimeOptimizer {
            features: optimizer.features,
        }
    } else {
        let optimizer = RuntimeOptimizer::new();
        *guard = Some(RuntimeOptimizer {
            features: optimizer.features,
        });
        optimizer
    }
}
